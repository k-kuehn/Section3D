//! Offline 3D viewer: one self-contained HTML file (WebGL2, no libraries)
//! showing the assembled model, an exploded view and the assembly steps.

use std::fmt::Write;

use glam::Vec3;

use crate::mesh::Mesh;

pub struct ViewerPart<'a> {
    pub label: u32,
    pub mesh: &'a Mesh,
    pub color: [f32; 3],
    /// Assembly step (0-based) and insertion direction.
    pub step: usize,
    pub insert: Vec3,
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

pub fn write(title: &str, parts: &[ViewerPart]) -> String {
    let mut data = String::from("[");
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            data.push(',');
        }
        let pos: &[u8] = bytemuck::cast_slice(&p.mesh.verts);
        let idx: &[u8] = bytemuck::cast_slice(&p.mesh.tris);
        let c = p.mesh.bbox().center();
        let _ = write!(
            data,
            "{{\"l\":{},\"c\":[{:.3},{:.3},{:.3}],\"s\":{},\"d\":[{:.3},{:.3},{:.3}],\"m\":[{:.2},{:.2},{:.2}],\"p\":\"{}\",\"i\":\"{}\"}}",
            p.label,
            p.color[0],
            p.color[1],
            p.color[2],
            p.step,
            p.insert.x,
            p.insert.y,
            p.insert.z,
            c.x,
            c.y,
            c.z,
            base64(pos),
            base64(idx)
        );
    }
    data.push(']');
    TEMPLATE.replace("__TITLE__", &title.replace('<', "&lt;")).replace("__DATA__", &data)
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
<div id="view"><canvas id="c"></canvas><div id="step"></div><div id="hint">drag to orbit · right-drag to pan · scroll to zoom · click a part</div></div></div>
<script>
const parts=__DATA__;
function b64(s){const b=atob(s),u=new Uint8Array(b.length);for(let i=0;i<b.length;i++)u[i]=b.charCodeAt(i);return u.buffer}
const cv=document.getElementById('c'),gl=cv.getContext('webgl2',{antialias:true});
if(!gl){document.getElementById('step').textContent='WebGL2 is not available in this browser.'}
const vs=`#version 300 es
in vec3 p;uniform mat4 mvp;uniform vec3 off;out vec3 w;void main(){w=p+off;gl_Position=mvp*vec4(w,1.0);}`;
const fs=`#version 300 es
precision highp float;in vec3 w;uniform vec3 col;uniform vec3 eye;uniform float sel;out vec4 o;
void main(){vec3 n=normalize(cross(dFdx(w),dFdy(w)));vec3 v=normalize(eye-w);if(dot(n,v)<0.0)n=-n;
float d=0.3+0.7*max(dot(n,normalize(v+vec3(0.2,0.1,0.5))),0.0);vec3 c=col*d;if(sel>0.5)c=mix(c,vec3(1.0,0.42,0.17),0.55);o=vec4(pow(c,vec3(0.95)),1.0);}`;
function sh(t,s){const x=gl.createShader(t);gl.shaderSource(x,s);gl.compileShader(x);return x}
const pr=gl.createProgram();gl.attachShader(pr,sh(gl.VERTEX_SHADER,vs));gl.attachShader(pr,sh(gl.FRAGMENT_SHADER,fs));gl.linkProgram(pr);gl.useProgram(pr);
const U=n=>gl.getUniformLocation(pr,n);let lo=[1e9,1e9,1e9],hi=[-1e9,-1e9,-1e9];
for(const p of parts){const pos=new Float32Array(b64(p.p)),idx=new Uint32Array(b64(p.i));p.n=idx.length;
for(let i=0;i<pos.length;i+=3)for(let k=0;k<3;k++){lo[k]=Math.min(lo[k],pos[i+k]);hi[k]=Math.max(hi[k],pos[i+k])}
p.vao=gl.createVertexArray();gl.bindVertexArray(p.vao);const vb=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,vb);gl.bufferData(gl.ARRAY_BUFFER,pos,gl.STATIC_DRAW);
gl.enableVertexAttribArray(0);gl.vertexAttribPointer(0,3,gl.FLOAT,false,0,0);const ib=gl.createBuffer();gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,ib);gl.bufferData(gl.ELEMENT_ARRAY_BUFFER,idx,gl.STATIC_DRAW);}
const ctr=[0,1,2].map(k=>(lo[k]+hi[k])/2),rad=Math.hypot(hi[0]-lo[0],hi[1]-lo[1],hi[2]-lo[2])/2||1;
let yaw=-0.8,pitch=0.45,dist=rad*3,tgt=ctr.slice(),sel=-1;const nsteps=Math.max(...parts.map(p=>p.s))+1;
const asm=document.getElementById('asm');asm.max=nsteps;asm.value=nsteps;document.getElementById('count').textContent=parts.length+' parts';
function mul(a,b){const r=new Array(16).fill(0);for(let i=0;i<4;i++)for(let j=0;j<4;j++)for(let k=0;k<4;k++)r[j*4+i]+=a[k*4+i]*b[j*4+k];return r}
function persp(f,a,n,fa){const t=1/Math.tan(f/2);return[t/a,0,0,0,0,t,0,0,0,0,fa/(n-fa),-1,0,0,n*fa/(n-fa),0]}
function look(e,c,u){let z=[e[0]-c[0],e[1]-c[1],e[2]-c[2]],l=Math.hypot(...z);z=z.map(v=>v/l);let x=[u[1]*z[2]-u[2]*z[1],u[2]*z[0]-u[0]*z[2],u[0]*z[1]-u[1]*z[0]];l=Math.hypot(...x);x=x.map(v=>v/l);
const y=[z[1]*x[2]-z[2]*x[1],z[2]*x[0]-z[0]*x[2],z[0]*x[1]-z[1]*x[0]];const d=(a,b)=>a[0]*b[0]+a[1]*b[1]+a[2]*b[2];return[x[0],y[0],z[0],0,x[1],y[1],z[1],0,x[2],y[2],z[2],0,-d(x,e),-d(y,e),-d(z,e),1]}
function eye(){return[tgt[0]+dist*Math.cos(pitch)*Math.cos(yaw),tgt[1]+dist*Math.cos(pitch)*Math.sin(yaw),tgt[2]+dist*Math.sin(pitch)]}
function draw(){const w=cv.clientWidth*devicePixelRatio,h=cv.clientHeight*devicePixelRatio;if(cv.width!==w||cv.height!==h){cv.width=w;cv.height=h}
gl.viewport(0,0,w,h);gl.clearColor(0.075,0.085,0.105,1);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);gl.enable(gl.DEPTH_TEST);
const e=eye(),mvp=mul(persp(0.6,w/h,Math.max(dist-rad*2,rad*0.01),dist+rad*4),look(e,tgt,[0,0,1]));gl.uniformMatrix4fv(U('mvp'),false,mvp);gl.uniform3fv(U('eye'),e);
const ex=+document.getElementById('explode').value,st=+asm.value-1;
for(const [i,p] of parts.entries()){if(p.s>st)continue;let o=[0,1,2].map(k=>(p.m[k]-ctr[k])*ex*0.6);
if(p.s===st&&st<nsteps-1)o=o.map((v,k)=>v-p.d[k]*rad*0.15);gl.uniform3fv(U('off'),o);gl.uniform3fv(U('col'),p.c);gl.uniform1f(U('sel'),i===sel||(p.s===st&&st<nsteps-1)?1:0);
gl.bindVertexArray(p.vao);gl.drawElements(gl.TRIANGLES,p.n,gl.UNSIGNED_INT,0)}
const cur=parts.find(p=>p.s===st);document.getElementById('stepv').textContent=(st+1)+' / '+nsteps;
document.getElementById('step').textContent=st<nsteps-1&&cur?'Step '+(st+1)+': add part #'+cur.l:''}
let drag=null;cv.oncontextmenu=e=>e.preventDefault();cv.onpointerdown=e=>{drag={x:e.clientX,y:e.clientY,b:e.button,m:0};cv.setPointerCapture(e.pointerId)};
cv.onpointermove=e=>{if(!drag)return;const dx=e.clientX-drag.x,dy=e.clientY-drag.y;drag.x=e.clientX;drag.y=e.clientY;drag.m+=Math.abs(dx)+Math.abs(dy);
if(drag.b===0){yaw-=dx*0.008;pitch=Math.max(-1.55,Math.min(1.55,pitch+dy*0.008))}else{const s=dist*0.0015,cy=Math.cos(yaw),sy=Math.sin(yaw),sp=Math.sin(pitch),cp=Math.cos(pitch);
const r=[-sy,cy,0],u=[-sp*cy,-sp*sy,cp];for(let k=0;k<3;k++)tgt[k]+=(-r[k]*dx+u[k]*dy)*s}draw()};
cv.onpointerup=()=>{drag=null};cv.onwheel=e=>{e.preventDefault();dist*=Math.exp(e.deltaY*0.001);draw()};
document.getElementById('explode').oninput=draw;asm.oninput=draw;
const list=document.getElementById('list');parts.slice().sort((a,b)=>a.l-b.l).forEach(p=>{const r=document.createElement('div');r.className='row';
r.innerHTML='<span class="sw" style="background:rgb('+p.c.map(v=>v*255|0)+')"></span>#'+p.l+' <span style="color:var(--weak);margin-left:auto">step '+(p.s+1)+'</span>';
r.onclick=()=>{sel=parts.indexOf(p);document.querySelectorAll('.row').forEach(x=>x.classList.remove('sel'));r.classList.add('sel');draw()};list.appendChild(r)});
addEventListener('resize',draw);draw();
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
}
