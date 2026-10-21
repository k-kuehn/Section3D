//! Offline 3D viewer: one self-contained HTML file (WebGL2, no libraries)
//! showing the assembled model, an exploded view and the assembly steps.
//!
//! Pieces are stored compactly so they keep their full detail: positions as
//! 16-bit offsets within each piece's box, smooth normals (split at sharp
//! edges) as bytes, all deflated into one blob the browser inflates itself.

use std::collections::HashMap;
use std::fmt::Write;

use glam::Vec3;

use crate::mesh::Mesh;

pub struct ViewerPart<'a> {
    pub label: u32,
    pub mesh: &'a Mesh,
    pub color: [f32; 3],
}

/// One assembly step: two parts (by piece label) joined into a section.
pub struct ViewerStep {
    pub name: String,
    pub a_name: String,
    pub b_name: String,
    pub a: Vec<u32>,
    pub b: Vec<u32>,
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Faces meeting at more than this keep a sharp edge (degrees).
const CREASE_DEG: f32 = 40.0;

/// Vertices with smooth normals: a vertex is shared by the faces around it
/// that lie within the crease angle of each other, and split elsewhere.
fn smooth(mesh: &Mesh) -> (Vec<Vec3>, Vec<Vec3>, Vec<u32>) {
    let fnorm: Vec<Vec3> = (0..mesh.tris.len()).map(|t| mesh.tri_cross(t)).collect();
    let unit: Vec<Vec3> = fnorm.iter().map(|n| n.normalize_or_zero()).collect();
    // Faces around each vertex.
    let mut start = vec![0u32; mesh.verts.len() + 1];
    for t in &mesh.tris {
        for &v in t {
            start[v as usize + 1] += 1;
        }
    }
    for i in 0..mesh.verts.len() {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut faces = vec![0u32; mesh.tris.len() * 3];
    for (f, t) in mesh.tris.iter().enumerate() {
        for &v in t {
            faces[fill[v as usize] as usize] = f as u32;
            fill[v as usize] += 1;
        }
    }
    let cos = CREASE_DEG.to_radians().cos();
    let (mut pos, mut nrm, mut idx) = (Vec::new(), Vec::new(), Vec::with_capacity(mesh.tris.len() * 3));
    let mut made: HashMap<(u32, [i8; 3]), u32> = HashMap::new();
    for (f, t) in mesh.tris.iter().enumerate() {
        for &v in t {
            let around = &faces[start[v as usize] as usize..start[v as usize + 1] as usize];
            let n = around.iter().filter(|&&g| unit[g as usize].dot(unit[f]) >= cos).fold(Vec3::ZERO, |a, &g| a + fnorm[g as usize]).normalize_or(unit[f]);
            let q = quantize(n);
            let i = *made.entry((v, q)).or_insert_with(|| {
                pos.push(mesh.verts[v as usize]);
                nrm.push(n);
                pos.len() as u32 - 1
            });
            idx.push(i);
        }
    }
    (pos, nrm, idx)
}

fn quantize(n: Vec3) -> [i8; 3] {
    n.to_array().map(|x| (x * 127.0).round().clamp(-127.0, 127.0) as i8)
}

pub fn write(title: &str, parts: &[ViewerPart], steps: &[ViewerStep]) -> String {
    let mut blob: Vec<u8> = Vec::new();
    let mut meta = String::from("[");
    for (i, p) in parts.iter().enumerate() {
        let (pos, nrm, idx) = smooth(p.mesh);
        let bb = p.mesh.bbox();
        let scale = (bb.size() / 65535.0).max(Vec3::splat(1e-6));
        let wide = pos.len() > 65535;
        let off = blob.len();
        for v in &pos {
            for k in 0..3 {
                let q = ((v[k] - bb.min[k]) / scale[k]).round().clamp(0.0, 65535.0) as u16;
                blob.extend_from_slice(&q.to_le_bytes());
            }
        }
        blob.resize(blob.len().next_multiple_of(4), 0);
        let noff = blob.len();
        for n in &nrm {
            let q = quantize(*n);
            blob.extend_from_slice(&[q[0] as u8, q[1] as u8, q[2] as u8, 0]);
        }
        let ioff = blob.len();
        for &x in &idx {
            if wide {
                blob.extend_from_slice(&x.to_le_bytes());
            } else {
                blob.extend_from_slice(&(x as u16).to_le_bytes());
            }
        }
        blob.resize(blob.len().next_multiple_of(4), 0);
        let c = bb.center();
        if i > 0 {
            meta.push(',');
        }
        let _ = write!(
            meta,
            "{{\"l\":{},\"c\":[{:.3},{:.3},{:.3}],\"m\":[{:.2},{:.2},{:.2}],\"o\":[{},{},{}],\"s\":[{},{},{}],\"nv\":{},\"ni\":{},\"po\":{},\"no\":{},\"io\":{},\"w\":{}}}",
            p.label,
            p.color[0],
            p.color[1],
            p.color[2],
            c.x,
            c.y,
            c.z,
            bb.min.x,
            bb.min.y,
            bb.min.z,
            scale.x,
            scale.y,
            scale.z,
            pos.len(),
            idx.len(),
            off,
            noff,
            ioff,
            wide as u8
        );
    }
    meta.push(']');
    let esc = |t: &str| t.replace(['\\', '"', '<'], "");
    let mut st = String::from("[");
    for (i, s) in steps.iter().enumerate() {
        if i > 0 {
            st.push(',');
        }
        let list = |v: &[u32]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
        let _ = write!(st, "{{\"n\":\"{}\",\"an\":\"{}\",\"bn\":\"{}\",\"a\":[{}],\"b\":[{}]}}", esc(&s.name), esc(&s.a_name), esc(&s.b_name), list(&s.a), list(&s.b));
    }
    st.push(']');
    let data = base64(&miniz_oxide::deflate::compress_to_vec_zlib(&blob, 6));
    TEMPLATE.replace("__TITLE__", &title.replace('<', "&lt;")).replace("__STEPS__", &st).replace("__META__", &meta).replace("__DATA__", &data)
}

const TEMPLATE: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>__TITLE__ · Section3D viewer</title>
<style>
:root{--bg:#0f1115;--panel:#15181e;--card:#1b1f27;--line:#2a313d;--text:#d6dde7;--weak:#8c98a9;--acc:#ff6b2c}
*{box-sizing:border-box}html,body{margin:0;height:100%;background:var(--bg);color:var(--text);font:14px/1.4 system-ui,Segoe UI,sans-serif}
#app{display:flex;height:100%}#side{width:280px;background:var(--panel);border-right:1px solid var(--line);display:flex;flex-direction:column}
h1{font-size:17px;margin:14px 16px 2px}h1 b{color:var(--acc)}.sub{color:var(--weak);margin:0 16px 10px;font-size:12px}
.ctl{padding:8px 16px;border-top:1px solid var(--line)}.ctl label{display:block;color:var(--weak);font-size:12px;margin-bottom:4px}
input[type=range]{width:100%;accent-color:var(--acc)}#list{flex:1;overflow:auto;padding:4px 8px}
.row{display:flex;align-items:center;gap:8px;padding:5px 8px;border-radius:6px;cursor:pointer}.row:hover{background:var(--card)}
.row.sel{background:#3a2216;outline:1px solid var(--acc)}.sw{width:10px;height:10px;border-radius:3px}
#view{flex:1;position:relative}canvas{width:100%;height:100%;display:block}#step{position:absolute;left:14px;top:12px;font-weight:600}
#hint{position:absolute;left:12px;bottom:8px;color:var(--weak);font:11px monospace}
@media (max-width:700px){#app{flex-direction:column}#side{width:100%;height:42%}}
</style></head><body><div id="app"><div id="side">
<h1>Section<b>3D</b></h1><div class="sub">__TITLE__ · <span id="count"></span></div>
<div class="ctl"><label>Explode</label><input id="explode" type="range" min="0" max="1" step="0.01" value="0"></div>
<div class="ctl"><label>Assembly step <span id="stepv"></span></label><input id="asm" type="range" min="1" value="1"></div>
<div id="list"></div></div>
<div id="view"><canvas id="c"></canvas><div id="step">Loading…</div><div id="hint">drag to orbit · right-drag to pan · scroll to zoom · click a part</div></div></div>
<script>
const parts=__META__;const steps=__STEPS__;const packed="__DATA__";
const cv=document.getElementById('c'),gl=cv.getContext('webgl2',{antialias:true}),msg=document.getElementById('step');
const vs=`#version 300 es
in vec3 q;in vec3 nq;uniform mat4 mvp;uniform vec3 org;uniform vec3 scl;uniform vec3 off;out vec3 w;out vec3 n;
void main(){w=org+q*65535.0*scl+off;n=nq;gl_Position=mvp*vec4(w,1.0);}`;
const fs=`#version 300 es
precision highp float;in vec3 w;in vec3 n;uniform vec3 col;uniform vec3 eye;uniform float sel;out vec4 o;
void main(){vec3 v=normalize(eye-w);vec3 m=normalize(n);if(!gl_FrontFacing)m=-m;
vec3 k=normalize(v+vec3(0.25,0.15,0.6));float d=0.28+0.6*max(dot(m,k),0.0)+0.12*max(dot(m,normalize(vec3(-0.5,-0.3,0.4))),0.0);
float sp=pow(max(dot(m,normalize(k+v)),0.0),40.0)*0.12;vec3 c=col;if(sel>0.5)c=mix(c,vec3(1.0,0.42,0.17),0.6);
c=c*d+vec3(sp);if(sel>0.5)c+=vec3(1.0,0.42,0.17)*pow(1.0-abs(dot(m,v)),3.0)*0.6;o=vec4(pow(c,vec3(0.95)),1.0);}`;
let yaw=-0.8,pitch=0.45,dist=1,tgt=[0,0,0],sel=-1,rad=1,ctr=[0,0,0],ready=false;
function sh(t,s){const x=gl.createShader(t);gl.shaderSource(x,s);gl.compileShader(x);return x}
async function load(){
if(!gl){msg.textContent='WebGL2 is not available in this browser.';return}
if(typeof DecompressionStream==='undefined'){msg.textContent='This browser is too old to open the viewer (no DecompressionStream).';return}
const res=await fetch('data:application/octet-stream;base64,'+packed);
const buf=await new Response(res.body.pipeThrough(new DecompressionStream('deflate'))).arrayBuffer();
const pr=gl.createProgram();gl.attachShader(pr,sh(gl.VERTEX_SHADER,vs));gl.attachShader(pr,sh(gl.FRAGMENT_SHADER,fs));
gl.bindAttribLocation(pr,0,'q');gl.bindAttribLocation(pr,1,'nq');gl.linkProgram(pr);gl.useProgram(pr);window.U=n=>gl.getUniformLocation(pr,n);
let lo=[1e9,1e9,1e9],hi=[-1e9,-1e9,-1e9];
for(const p of parts){for(let k=0;k<3;k++){lo[k]=Math.min(lo[k],p.o[k]);hi[k]=Math.max(hi[k],p.o[k]+p.s[k]*65535)}
p.vao=gl.createVertexArray();gl.bindVertexArray(p.vao);
const vb=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,vb);gl.bufferData(gl.ARRAY_BUFFER,new Uint16Array(buf,p.po,p.nv*3),gl.STATIC_DRAW);
gl.enableVertexAttribArray(0);gl.vertexAttribPointer(0,3,gl.UNSIGNED_SHORT,true,6,0);
const nb=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,nb);gl.bufferData(gl.ARRAY_BUFFER,new Int8Array(buf,p.no,p.nv*4),gl.STATIC_DRAW);
gl.enableVertexAttribArray(1);gl.vertexAttribPointer(1,3,gl.BYTE,true,4,0);
const ib=gl.createBuffer();gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,ib);
gl.bufferData(gl.ELEMENT_ARRAY_BUFFER,p.w?new Uint32Array(buf,p.io,p.ni):new Uint16Array(buf,p.io,p.ni),gl.STATIC_DRAW);p.t=p.w?gl.UNSIGNED_INT:gl.UNSIGNED_SHORT}
ctr=[0,1,2].map(k=>(lo[k]+hi[k])/2);rad=Math.hypot(hi[0]-lo[0],hi[1]-lo[1],hi[2]-lo[2])/2||1;dist=rad*3;tgt=ctr.slice();ready=true;draw()}
const nsteps=steps.length+1;
for(const p of parts){p.s2=steps.findIndex(s=>s.a.includes(p.l)||s.b.includes(p.l));if(p.s2<0)p.s2=steps.length}
const asm=document.getElementById('asm');asm.max=nsteps;asm.value=nsteps;document.getElementById('count').textContent=parts.length+' parts';
function mul(a,b){const r=new Array(16).fill(0);for(let i=0;i<4;i++)for(let j=0;j<4;j++)for(let k=0;k<4;k++)r[j*4+i]+=a[k*4+i]*b[j*4+k];return r}
function persp(f,a,n,fa){const t=1/Math.tan(f/2);return[t/a,0,0,0,0,t,0,0,0,0,fa/(n-fa),-1,0,0,n*fa/(n-fa),0]}
function look(e,c,u){let z=[e[0]-c[0],e[1]-c[1],e[2]-c[2]],l=Math.hypot(...z);z=z.map(v=>v/l);let x=[u[1]*z[2]-u[2]*z[1],u[2]*z[0]-u[0]*z[2],u[0]*z[1]-u[1]*z[0]];l=Math.hypot(...x);x=x.map(v=>v/l);
const y=[z[1]*x[2]-z[2]*x[1],z[2]*x[0]-z[0]*x[2],z[0]*x[1]-z[1]*x[0]];const d=(a,b)=>a[0]*b[0]+a[1]*b[1]+a[2]*b[2];return[x[0],y[0],z[0],0,x[1],y[1],z[1],0,x[2],y[2],z[2],0,-d(x,e),-d(y,e),-d(z,e),1]}
function eye(){return[tgt[0]+dist*Math.cos(pitch)*Math.cos(yaw),tgt[1]+dist*Math.cos(pitch)*Math.sin(yaw),tgt[2]+dist*Math.sin(pitch)]}
function draw(){if(!ready)return;const w=cv.clientWidth*devicePixelRatio,h=cv.clientHeight*devicePixelRatio;if(cv.width!==w||cv.height!==h){cv.width=w;cv.height=h}
gl.viewport(0,0,w,h);gl.clearColor(0.075,0.085,0.105,1);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);gl.enable(gl.DEPTH_TEST);
const e=eye(),mvp=mul(persp(0.6,w/h,Math.max(dist-rad*2,dist*0.002),dist+rad*4),look(e,tgt,[0,0,1]));gl.uniformMatrix4fv(U('mvp'),false,mvp);gl.uniform3fv(U('eye'),e);
const ex=+document.getElementById('explode').value,st=+asm.value-1;
const S=st<steps.length?steps[st]:null;
for(const [i,p] of parts.entries()){const o=[0,1,2].map(k=>(p.m[k]-ctr[k])*ex*0.6);let c=p.c,hl=i===sel;
if(S){const inA=S.a.includes(p.l),inB=S.b.includes(p.l);if(!inA&&!inB)c=c.map(v=>v*0.45+0.04);if(inB)hl=true}
gl.uniform3fv(U('off'),o);gl.uniform3fv(U('org'),p.o);gl.uniform3fv(U('scl'),p.s);gl.uniform3fv(U('col'),c);gl.uniform1f(U('sel'),hl?1:0);
gl.bindVertexArray(p.vao);gl.drawElements(gl.TRIANGLES,p.ni,p.t,0)}
document.getElementById('stepv').textContent=(st+1)+' / '+nsteps;
msg.textContent=S?'Step '+(st+1)+': join '+S.an+' and '+S.bn+' into section '+S.n:'Assembled'}
let drag=null;cv.oncontextmenu=e=>e.preventDefault();cv.onpointerdown=e=>{drag={x:e.clientX,y:e.clientY,b:e.button,m:0};cv.setPointerCapture(e.pointerId)};
cv.onpointermove=e=>{if(!drag)return;const dx=e.clientX-drag.x,dy=e.clientY-drag.y;drag.x=e.clientX;drag.y=e.clientY;drag.m+=Math.abs(dx)+Math.abs(dy);
if(drag.b===0){yaw-=dx*0.008;pitch=Math.max(-1.55,Math.min(1.55,pitch+dy*0.008))}else{const s=dist*0.0015,cy=Math.cos(yaw),sy=Math.sin(yaw),sp=Math.sin(pitch),cp=Math.cos(pitch);
const r=[-sy,cy,0],u=[-sp*cy,-sp*sy,cp];for(let k=0;k<3;k++)tgt[k]+=(-r[k]*dx+u[k]*dy)*s}draw()};
cv.onpointerup=()=>{drag=null};cv.onwheel=e=>{e.preventDefault();dist*=Math.exp(e.deltaY*0.001);draw()};
document.getElementById('explode').oninput=draw;asm.oninput=draw;
const list=document.getElementById('list');parts.slice().sort((a,b)=>a.l-b.l).forEach(p=>{const r=document.createElement('div');r.className='row';
r.innerHTML='<span class="sw" style="background:rgb('+p.c.map(v=>v*255|0)+')"></span>#'+p.l+' <span style="color:var(--weak);margin-left:auto">step '+(p.s2+1)+'</span>';
r.onclick=()=>{sel=parts.indexOf(p);document.querySelectorAll('.row').forEach(x=>x.classList.remove('sel'));r.classList.add('sel');draw()};list.appendChild(r)});
addEventListener('resize',draw);load().catch(e=>{msg.textContent='Could not open the model: '+e});
</script></body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn cube_keeps_sharp_edges_and_sphere_is_smooth() {
        // A cube splits every corner three ways (24 vertices); a sphere shares them.
        let (pos, nrm, idx) = smooth(&crate::mesh::shapes::cube(Vec3::splat(10.0)));
        assert_eq!(pos.len(), 24);
        assert_eq!(idx.len(), 36);
        assert!(nrm.iter().all(|n| n.abs().max_element() > 0.99));
        let s = crate::mesh::shapes::uv_sphere(50.0, 32, 16);
        let (pos, nrm, _) = smooth(&s);
        assert!(pos.len() <= s.verts.len() + 64, "{} vs {}", pos.len(), s.verts.len());
        assert!(pos.iter().zip(&nrm).all(|(p, n)| p.normalize().dot(*n) > 0.97));
    }
}
