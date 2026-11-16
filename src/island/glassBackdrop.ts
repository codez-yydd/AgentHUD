// Gaussian blur kernel and rounded-rectangle edge refraction for the WebGL backdrop.
// The island's animated CSS path clips the output, including its liquid bridge.

export interface GlassBackdropGeometry {
  width: number;
  height: number;
  x: number;
  y: number;
  bodyWidth: number;
  bodyHeight: number;
  radius: number;
  strength?: number;
}

export interface GlassBackdrop {
  setFrame(source: TexImageSource): void;
  draw(geometry: GlassBackdropGeometry): void;
  dispose(): void;
}

const vertexSource = `#version 300 es
out vec2 uv;
void main() {
  uv = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}`;

const blurSource = `#version 300 es
precision highp float;
in vec2 uv;
uniform sampler2D image;
uniform vec2 dir;
uniform bool flipSource;
out vec4 color;
void main() {
  vec2 point = flipSource ? vec2(uv.x, 1.0 - uv.y) : uv;
  color = texture(image, point) * 0.4026
    + (texture(image, point + dir) + texture(image, point - dir)) * 0.2442
    + (texture(image, point + 2.0 * dir) + texture(image, point - 2.0 * dir)) * 0.0545;
}`;

const lensSource = `#version 300 es
precision highp float;
in vec2 uv;
uniform sampler2D image;
uniform vec2 viewSize;
uniform vec4 body;
uniform float cornerRadius;
uniform float strength;
out vec4 color;

vec2 toUV(vec2 point) {
  return vec2(point.x / viewSize.x, 1.0 - point.y / viewSize.y);
}

void main() {
  vec2 point = vec2(uv.x, 1.0 - uv.y) * viewSize;
  vec2 halfSize = body.zw * 0.5;
  vec2 p = point - body.xy - halfSize;
  float halfMin = min(halfSize.x, halfSize.y);
  float radius = clamp(cornerRadius, 0.0, halfMin);
  vec2 q = abs(p) - halfSize + radius;
  float sd = min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - radius;
  float depth = -sd;
  float bezel = min(34.0, 0.75 * halfMin);
  vec2 normal = vec2(0.0);
  vec2 displacement = vec2(0.0);
  float bump = 0.0;
  float edgeWeight = 0.0;

  if (depth > 0.0 && depth < bezel) {
    float soft = max(radius * 0.8, 1.0);
    vec2 clamped = 0.5 * (q + sqrt(q * q + soft * soft));
    normal = clamped / max(length(clamped), 0.0001) * sign(p + vec2(0.000001));
    float t = depth / bezel;
    edgeWeight = (1.0 - t) * (1.0 - t);
    float peak = clamp(t < 0.62 ? t / 0.62 : (1.0 - t) / 0.38, 0.0, 1.0);
    bump = peak * peak * (3.0 - 2.0 * peak);
    displacement = normal * (bump * 0.289 * bezel * strength);
  }

  vec3 rgb;
  float displacementSq = dot(displacement, displacement);
  if (displacementSq > 0.25) {
    vec2 tangent = vec2(normal.y, -normal.x) * sqrt(displacementSq);
    float spread = 0.05 + 0.10 * bump;
    const float lateral[5] = float[5](-0.36, 0.18, 0.0, -0.18, 0.36);
    const float weights[5] = float[5](0.14, 0.22, 0.28, 0.22, 0.14);
    rgb = vec3(0.0);
    for (int i = 0; i < 5; i++) {
      vec2 offset = displacement * (1.0 + float(i - 2) * spread) + tangent * lateral[i];
      rgb += weights[i] * vec3(
        texture(image, toUV(point + offset)).r,
        texture(image, toUV(point + offset * 0.90)).g,
        texture(image, toUV(point + offset * 0.80)).b
      );
    }
  } else {
    rgb = texture(image, toUV(point + displacement)).rgb;
  }

  float luminance = dot(rgb, vec3(0.213, 0.715, 0.072));
  rgb = mix(vec3(luminance), rgb, 1.4);
  float lighting = dot(normal, vec2(-0.40, -0.92));
  rgb += strength * edgeWeight * (0.09 * max(lighting, 0.0) - 0.04 * max(-lighting, 0.0));
  color = vec4(clamp(rgb, 0.0, 1.0), 1.0);
}`;

function compile(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("Unable to create glass shader");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const message = gl.getShaderInfoLog(shader);
    gl.deleteShader(shader);
    throw new Error(message || "Unable to compile glass shader");
  }
  return shader;
}

function link(gl: WebGL2RenderingContext, vertex: WebGLShader, source: string): WebGLProgram {
  const fragment = compile(gl, gl.FRAGMENT_SHADER, source);
  try {
    const program = gl.createProgram();
    if (!program) throw new Error("Unable to create glass shader program");
    gl.attachShader(program, vertex);
    gl.attachShader(program, fragment);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      const message = gl.getProgramInfoLog(program);
      gl.deleteProgram(program);
      throw new Error(message || "Unable to link glass shader");
    }
    return program;
  } finally {
    gl.deleteShader(fragment);
  }
}

function createRenderer(gl: WebGL2RenderingContext) {
  const textures: WebGLTexture[] = [];
  const framebuffers: WebGLFramebuffer[] = [];
  const programs: WebGLProgram[] = [];
  const dispose = () => {
    framebuffers.forEach((framebuffer) => gl.deleteFramebuffer(framebuffer));
    textures.forEach((texture) => gl.deleteTexture(texture));
    programs.forEach((program) => gl.deleteProgram(program));
  };

  const createTexture = () => {
    const texture = gl.createTexture();
    if (!texture) throw new Error("Unable to create glass texture");
    textures.push(texture);
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    return texture;
  };

  const createTarget = () => {
    const texture = createTexture();
    const framebuffer = gl.createFramebuffer();
    if (!framebuffer) throw new Error("Unable to create glass framebuffer");
    framebuffers.push(framebuffer);
    gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texture, 0);
    return { texture, framebuffer };
  };

  try {
    const vertex = compile(gl, gl.VERTEX_SHADER, vertexSource);
    try {
      programs.push(link(gl, vertex, blurSource));
      programs.push(link(gl, vertex, lensSource));
    } finally {
      gl.deleteShader(vertex);
    }
    const [blur, lens] = programs;
    const frame = createTexture();
    const horizontal = createTarget();
    const vertical = createTarget();
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    return {
      blur, lens, frame, horizontal, vertical, dispose,
      direction: gl.getUniformLocation(blur, "dir"),
      flipSource: gl.getUniformLocation(blur, "flipSource"),
      viewSize: gl.getUniformLocation(lens, "viewSize"),
      body: gl.getUniformLocation(lens, "body"),
      radius: gl.getUniformLocation(lens, "cornerRadius"),
      strength: gl.getUniformLocation(lens, "strength"),
    };
  } catch (error) {
    dispose();
    throw error;
  }
}

export function createGlassBackdrop(canvas: HTMLCanvasElement): GlassBackdrop | null {
  let gl: WebGL2RenderingContext | null;
  try {
    gl = canvas.getContext("webgl2", { alpha: false, antialias: false, depth: false });
  } catch {
    return null;
  }
  if (!gl || gl.isContextLost()) return null;

  let renderer: ReturnType<typeof createRenderer> | null;
  try {
    renderer = createRenderer(gl);
  } catch (error) {
    console.warn("Unable to initialize island glass backdrop", error);
    return null;
  }

  let hasFrame = false;
  let needsBlur = true;
  let blurWidth = 0;
  let blurHeight = 0;
  let lastGeometry: GlassBackdropGeometry | undefined;
  let disposed = false;

  const handleContextLost = (event: Event) => {
    event.preventDefault();
    hasFrame = false;
    renderer = null;
  };
  const handleContextRestored = () => {
    try {
      renderer = createRenderer(gl);
      blurWidth = 0;
      blurHeight = 0;
      needsBlur = true;
    } catch (error) {
      console.warn("Unable to restore island glass backdrop", error);
    }
  };
  canvas.addEventListener("webglcontextlost", handleContextLost);
  canvas.addEventListener("webglcontextrestored", handleContextRestored);

  return {
    setFrame(source) {
      if (!renderer || disposed || gl.isContextLost()) return;
      gl.bindTexture(gl.TEXTURE_2D, renderer.frame);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source);
      hasFrame = true;
      needsBlur = true;
    },

    draw({ width, height, x, y, bodyWidth, bodyHeight, radius, strength = 1 }) {
      if (!renderer || !hasFrame || disposed || gl.isContextLost() || width <= 0 || height <= 0) return;
      const scale = Math.min(window.devicePixelRatio || 1, 2);
      const outputWidth = Math.max(1, Math.round(width * scale));
      const outputHeight = Math.max(1, Math.round(height * scale));
      if (canvas.width !== outputWidth || canvas.height !== outputHeight) {
        canvas.width = outputWidth;
        canvas.height = outputHeight;
        needsBlur = true;
      }

      if (!needsBlur && lastGeometry?.width === width && lastGeometry.height === height &&
        lastGeometry.x === x && lastGeometry.y === y && lastGeometry.bodyWidth === bodyWidth &&
        lastGeometry.bodyHeight === bodyHeight && lastGeometry.radius === radius && lastGeometry.strength === strength) {
        return;
      }

      const nextWidth = Math.ceil(outputWidth / 2);
      const nextHeight = Math.ceil(outputHeight / 2);
      if (blurWidth !== nextWidth || blurHeight !== nextHeight) {
        blurWidth = nextWidth;
        blurHeight = nextHeight;
        for (const { texture } of [renderer.horizontal, renderer.vertical]) {
          gl.bindTexture(gl.TEXTURE_2D, texture);
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, blurWidth, blurHeight, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
        }
        needsBlur = true;
      }

      if (needsBlur) {
        gl.useProgram(renderer.blur);
        gl.viewport(0, 0, blurWidth, blurHeight);
        gl.bindFramebuffer(gl.FRAMEBUFFER, renderer.horizontal.framebuffer);
        gl.bindTexture(gl.TEXTURE_2D, renderer.frame);
        gl.uniform1i(renderer.flipSource, 1);
        gl.uniform2f(renderer.direction, 5 / outputWidth, 0);
        gl.drawArrays(gl.TRIANGLES, 0, 3);

        gl.bindFramebuffer(gl.FRAMEBUFFER, renderer.vertical.framebuffer);
        gl.bindTexture(gl.TEXTURE_2D, renderer.horizontal.texture);
        gl.uniform1i(renderer.flipSource, 0);
        gl.uniform2f(renderer.direction, 0, 5 / outputHeight);
        gl.drawArrays(gl.TRIANGLES, 0, 3);
        needsBlur = false;
      }

      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.viewport(0, 0, outputWidth, outputHeight);
      gl.useProgram(renderer.lens);
      gl.bindTexture(gl.TEXTURE_2D, renderer.vertical.texture);
      gl.uniform2f(renderer.viewSize, width, height);
      gl.uniform4f(renderer.body, x, y, Math.max(1, bodyWidth), Math.max(1, bodyHeight));
      gl.uniform1f(renderer.radius, radius);
      gl.uniform1f(renderer.strength, Math.max(0, Math.min(1, strength)));
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      lastGeometry = { width, height, x, y, bodyWidth, bodyHeight, radius, strength };
    },

    dispose() {
      if (disposed) return;
      disposed = true;
      canvas.removeEventListener("webglcontextlost", handleContextLost);
      canvas.removeEventListener("webglcontextrestored", handleContextRestored);
      renderer?.dispose();
    },
  };
}
