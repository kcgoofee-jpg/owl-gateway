import posterUrl from "../../assets/splash-light-poster.png";

// Original Newsreader glyphs, white fill and smooth cold/warm reflected light.
export async function createSplashLight(canvas) {
  const W = 1050,
    H = 540,
    range = 128;
  let gl,
    program,
    texture,
    glowTexture,
    renderTime = 0;
  await document.fonts.load('166px "Newsreader Splash"');
  const mask = document.createElement("canvas");
  mask.width = W;
  mask.height = H;
  const ctx = mask.getContext("2d", { willReadFrequently: true });
  ctx.scale(1.5, 1.5);
  ctx.fillStyle = "white";
  ctx.textAlign = "center";
  ctx.font = '400 166px "Newsreader Splash"';
  ctx.fillText("Drawing", 350, 148);
  ctx.fillText("fantasy", 350, 310);
  const pixels = ctx.getImageData(0, 0, W, H).data;
  function distance(inside) {
    const d = new Float32Array(W * H);
    for (let i = 0; i < d.length; i++) d[i] = pixels[i * 4 + 3] > 127 === inside ? 0 : 1e6;
    const q = Math.SQRT2;
    for (let y = 0; y < H; y++)
      for (let x = 0; x < W; x++) {
        const i = y * W + x;
        let v = d[i];
        if (x) v = Math.min(v, d[i - 1] + 1);
        if (y) {
          v = Math.min(v, d[i - W] + 1);
          if (x) v = Math.min(v, d[i - W - 1] + q);
          if (x < W - 1) v = Math.min(v, d[i - W + 1] + q);
        }
        d[i] = v;
      }
    for (let y = H - 1; y >= 0; y--)
      for (let x = W - 1; x >= 0; x--) {
        const i = y * W + x;
        let v = d[i];
        if (x < W - 1) v = Math.min(v, d[i + 1] + 1);
        if (y < H - 1) {
          v = Math.min(v, d[i + W] + 1);
          if (x) v = Math.min(v, d[i + W - 1] + q);
          if (x < W - 1) v = Math.min(v, d[i + W + 1] + q);
        }
        d[i] = v;
      }
    return d;
  }
  const a = distance(false),
    b = distance(true),
    data = new Uint8Array(W * H * 4);
  for (let i = 0; i < a.length; i++) {
    const encoded = Math.round(Math.max(0, Math.min(1, 0.5 + (a[i] - b[i]) / range)) * 65535);
    data[i * 4] = encoded >> 8;
    data[i * 4 + 1] = encoded & 255;
    data[i * 4 + 2] = pixels[i * 4 + 3];
    data[i * 4 + 3] = 255;
  }

  // True separable Gaussian convolution: rounded halos, no distance-field facets.
  function blurMask(sigma) {
    const radius = Math.ceil(sigma * 3),
      kernel = new Float32Array(radius * 2 + 1);
    let sum = 0;
    for (let n = -radius; n <= radius; n++) {
      const v = Math.exp((-n * n) / (2 * sigma * sigma));
      kernel[n + radius] = v;
      sum += v;
    }
    for (let n = 0; n < kernel.length; n++) kernel[n] /= sum;
    const temp = new Float32Array(W * H),
      out = new Float32Array(W * H);
    for (let y = 0; y < H; y++)
      for (let x = 0; x < W; x++) {
        let v = 0;
        for (let k = -radius; k <= radius; k++) {
          const xx = x + k;
          if (xx >= 0 && xx < W) v += (pixels[(y * W + xx) * 4 + 3] / 255) * kernel[k + radius];
        }
        temp[y * W + x] = v;
      }
    for (let y = 0; y < H; y++)
      for (let x = 0; x < W; x++) {
        let v = 0;
        for (let k = -radius; k <= radius; k++) {
          const yy = y + k;
          if (yy >= 0 && yy < H) v += temp[yy * W + x] * kernel[k + radius];
        }
        out[y * W + x] = v;
      }
    return out;
  }
  const glowData = new Uint8Array(W * H * 4),
    blurA = blurMask(3),
    blurB = blurMask(10),
    blurC = blurMask(24);
  for (let i = 0; i < W * H; i++) {
    glowData[i * 4] = blurA[i] * 255;
    glowData[i * 4 + 1] = blurB[i] * 255;
    glowData[i * 4 + 2] = blurC[i] * 255;
    glowData[i * 4 + 3] = 255;
  }

  const vert = `#version 300 es
in vec2 position;out vec2 uv;void main(){uv=position*.5+.5;gl_Position=vec4(position,0,1);}`;
  const frag = `#version 300 es
precision highp float;uniform sampler2D field;uniform sampler2D glowField;uniform vec2 size;uniform float phase;uniform float reveal;uniform float completion;in vec2 uv;out vec4 outputColor;
float sd(vec2 p){vec2 v=texture(field,p).rg;return ((v.r*65280.+v.g*255.)/65535.-.5)*128.;}
vec3 light(vec2 p,vec3 normal){
 float t=phase;float diagonal=p.y+.19*sin(p.x*3.1+t*.7)+.16*p.x;
 float center=mix(-.1,1.35,t);float z=diagonal-center;
 // A broad reflected white softbox, with a narrow cold/warm refracted fringe.
 float core=exp(-pow(z/.13,2.));
 float cool=exp(-pow((z+.098)/.044,2.));float warm=exp(-pow((z-.095)/.028,2.));
 float edgeResponse=.35+.65*pow(max(0.,dot(normal,normalize(vec3(-.3,.7,.75)))),2.);
 vec3 c=vec3(.992,.988,1.)*core*1.38;
 c+=vec3(.604,.698,.863)*cool*1.05;
 c+=vec3(.941,.722,.576)*warm*.85;
 return c*edgeResponse;
}
void main(){vec2 p=vec2(uv.x,1.-uv.y);float d=sd(p);vec2 e=1./size;float dx=(sd(p+vec2(e.x,0))-sd(p-vec2(e.x,0)))*.5;float dy=(sd(p+vec2(0,e.y))-sd(p-vec2(0,e.y)))*.5;
 float inside=texture(field,p).b;float bevel=clamp(d/4.8,0.,1.);vec3 normal=normalize(vec3(-dx*(1.-bevel),-dy*(1.-bevel),.32+.68*bevel));
 vec3 white=vec3(253.,252.,255.)/255.;
 vec3 cool=vec3(.43,.66,1.);vec3 warm=vec3(1.,.70,.39);
 float front=-.25+1.75*phase;
 float wave=p.y+.045*sin(p.x*5.1);
 float lit=1.-smoothstep(front-.20,front+.20,wave);
 float band=exp(-pow((wave-front)/.105,2.));
 vec3 glass=vec3(.012,.014,.021);
 vec3 tint=mix(cool,warm,smoothstep(.30,.85,p.x));
 float bevelRim=exp(-max(d,0.)*.7);
 vec3 surface=mix(glass,white*.97,lit);
 surface*=mix(1.,.92+.08*normal.z,lit);
 float coldFringe=exp(-pow((wave-front-.055)/.045,2.));
 float warmFringe=exp(-pow((wave-front+.065)/.035,2.));
 surface+=cool*coldFringe*.48+warm*warmFringe*.24;
 surface+=tint*band*.20*bevelRim;
 surface=mix(surface,white,completion);
 vec3 g=texture(glowField,p).rgb;
 // Blur is computed from filled glyphs; it follows the actual smooth type.
 float halo=g.r*.16+g.g*.42+g.b*.55;
 float illuminated=mix(lit,1.,completion);
 vec3 bloom=mix(tint,white,.68)*halo*illuminated;
 vec3 col=surface*inside+bloom*(1.-inside);
 outputColor=vec4(col*reveal,1.);
}`;
  function shader(type, src) {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
    return s;
  }
  function setup() {
    gl = canvas.getContext("webgl2", {
      alpha: false,
      antialias: true,
      preserveDrawingBuffer: true,
      powerPreference: "low-power",
    });
    if (!gl) throw new Error("WebGL2 unavailable");
    program = gl.createProgram();
    gl.attachShader(program, shader(gl.VERTEX_SHADER, vert));
    gl.attachShader(program, shader(gl.FRAGMENT_SHADER, frag));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error("Link failed");
    gl.useProgram(program);
    const v = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, v);
    gl.bufferData(
      gl.ARRAY_BUFFER,
      new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]),
      gl.STATIC_DRAW,
    );
    const loc = gl.getAttribLocation(program, "position");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
    texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, W, H, 0, gl.RGBA, gl.UNSIGNED_BYTE, data);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.uniform1i(gl.getUniformLocation(program, "field"), 0);
    gl.activeTexture(gl.TEXTURE1);
    glowTexture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, glowTexture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, W, H, 0, gl.RGBA, gl.UNSIGNED_BYTE, glowData);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.uniform1i(gl.getUniformLocation(program, "glowField"), 1);
    gl.uniform2f(gl.getUniformLocation(program, "size"), W, H);
    resize();
  }
  function resize() {
    const r = canvas.getBoundingClientRect(),
      dpr = Math.min(devicePixelRatio || 1, 1.5);
    canvas.width = Math.min(2048, Math.round(r.width * dpr));
    canvas.height = Math.round((canvas.width * H) / W);
    gl.viewport(0, 0, canvas.width, canvas.height);
    draw(renderTime);
  }
  function draw(t) {
    renderTime = t;
    const phase = 0.16 + 0.574 * Math.min(t / 1.35, 1);
    gl.uniform1f(gl.getUniformLocation(program, "phase"), phase);
    const u = Math.max(0, Math.min(1, (t - 1.02) / 0.58));
    gl.uniform1f(gl.getUniformLocation(program, "completion"), u * u * (3 - 2 * u));
    gl.uniform1f(gl.getUniformLocation(program, "reveal"), 1);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
  }

  try {
    setup();
    return {
      draw,
      dispose() {
        gl.deleteTexture(texture);
        gl.deleteTexture(glowTexture);
        gl.deleteProgram(program);
        const ext = gl.getExtension("WEBGL_lose_context");
        if (ext) ext.loseContext();
      },
    };
  } catch (error) {
    if (gl) {
      const ext = gl.getExtension("WEBGL_lose_context");
      if (ext) ext.loseContext();
    }
    throw error;
  }
}
export { posterUrl };
