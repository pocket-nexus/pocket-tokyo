//! The part of OpenGL ES 3.0 this renderer calls, as the system's `libGLESv3.so` exports it.

#![allow(non_snake_case, dead_code, clippy::too_many_arguments)]

use core::ffi::{c_char, c_void};

pub type Enum = u32;

pub const TRIANGLES: Enum = 0x0004;
pub const DEPTH_BUFFER_BIT: Enum = 0x0100;
pub const STENCIL_BUFFER_BIT: Enum = 0x0400;
pub const COLOR_BUFFER_BIT: Enum = 0x4000;
pub const SRC_ALPHA: Enum = 0x0302;
pub const ONE: Enum = 1;
pub const ONE_MINUS_SRC_ALPHA: Enum = 0x0303;
pub const FRONT: Enum = 0x0404;
pub const BACK: Enum = 0x0405;
pub const CW: Enum = 0x0900;
pub const CCW: Enum = 0x0901;
pub const CULL_FACE: Enum = 0x0B44;
pub const DEPTH_TEST: Enum = 0x0B71;
pub const STENCIL_TEST: Enum = 0x0B90;
pub const BLEND: Enum = 0x0BE2;
pub const SCISSOR_TEST: Enum = 0x0C11;
pub const DITHER: Enum = 0x0BD0;
pub const LEQUAL: Enum = 0x0203;
pub const UNPACK_ALIGNMENT: Enum = 0x0CF5;
pub const TEXTURE_2D: Enum = 0x0DE1;
pub const BYTE: Enum = 0x1400;
pub const UNSIGNED_BYTE: Enum = 0x1401;
pub const SHORT: Enum = 0x1402;
pub const UNSIGNED_SHORT: Enum = 0x1403;
pub const UNSIGNED_INT: Enum = 0x1405;
pub const FLOAT: Enum = 0x1406;
pub const HALF_FLOAT: Enum = 0x140B;
pub const RED: Enum = 0x1903;
pub const RGB: Enum = 0x1907;
pub const RGBA: Enum = 0x1908;
pub const NEAREST: Enum = 0x2600;
pub const LINEAR: Enum = 0x2601;
pub const LINEAR_MIPMAP_NEAREST: Enum = 0x2701;
pub const LINEAR_MIPMAP_LINEAR: Enum = 0x2703;
pub const TEXTURE_MAG_FILTER: Enum = 0x2800;
pub const TEXTURE_MIN_FILTER: Enum = 0x2801;
pub const TEXTURE_WRAP_S: Enum = 0x2802;
pub const TEXTURE_WRAP_T: Enum = 0x2803;
pub const REPEAT: Enum = 0x2901;
pub const CLAMP_TO_EDGE: Enum = 0x812F;
pub const TEXTURE_MAX_LEVEL: Enum = 0x813D;
pub const TEXTURE_MAX_ANISOTROPY: Enum = 0x84FE;
pub const TEXTURE0: Enum = 0x84C0;
pub const UNSIGNED_SHORT_5_6_5: Enum = 0x8363;
pub const RGB565: Enum = 0x8D62;
pub const RGBA8: Enum = 0x8058;
pub const R16F: Enum = 0x822D;
pub const ARRAY_BUFFER: Enum = 0x8892;
pub const ELEMENT_ARRAY_BUFFER: Enum = 0x8893;
pub const STREAM_DRAW: Enum = 0x88E0;
pub const STATIC_DRAW: Enum = 0x88E4;
pub const DYNAMIC_DRAW: Enum = 0x88E8;
pub const FRAGMENT_SHADER: Enum = 0x8B30;
pub const VERTEX_SHADER: Enum = 0x8B31;
pub const COMPILE_STATUS: Enum = 0x8B81;
pub const LINK_STATUS: Enum = 0x8B82;
pub const COMPRESSED_RGB8_ETC2: Enum = 0x9274;
pub const COMPRESSED_RGBA8_ETC2_EAC: Enum = 0x9278;
pub const FRAMEBUFFER: Enum = 0x8D40;
pub const READ_FRAMEBUFFER: Enum = 0x8CA8;
pub const DRAW_FRAMEBUFFER: Enum = 0x8CA9;
pub const COLOR_ATTACHMENT0: Enum = 0x8CE0;
pub const FRAMEBUFFER_COMPLETE: Enum = 0x8CD5;
pub const COLOR: Enum = 0x1800;
pub const DEPTH: Enum = 0x1801;
pub const STENCIL: Enum = 0x1802;
pub const RENDERER: Enum = 0x1F01;
pub const VERSION: Enum = 0x1F02;
pub const TIME_ELAPSED: Enum = 0x88BF;
pub const QUERY_RESULT: Enum = 0x8866;
pub const QUERY_RESULT_AVAILABLE: Enum = 0x8867;
/// `QCOM_binning_control`.
pub const BINNING_CONTROL_HINT: Enum = 0x8FB0;
pub const RENDER_DIRECT_TO_FRAMEBUFFER: Enum = 0x8FB3;
pub const DONT_CARE: Enum = 0x1100;

extern "C" {
    pub fn glGetError() -> Enum;
    pub fn glGetString(name: Enum) -> *const u8;
    pub fn glEnable(cap: Enum);
    pub fn glDisable(cap: Enum);
    pub fn glHint(target: Enum, mode: Enum);
    pub fn glDepthFunc(func: Enum);
    pub fn glDepthMask(flag: u8);
    pub fn glColorMask(r: u8, g: u8, b: u8, a: u8);
    pub fn glCullFace(mode: Enum);
    pub fn glFrontFace(mode: Enum);
    pub fn glBlendFunc(s: Enum, d: Enum);
    pub fn glViewport(x: i32, y: i32, w: i32, h: i32);
    pub fn glClearColor(r: f32, g: f32, b: f32, a: f32);
    pub fn glClearDepthf(d: f32);
    pub fn glClear(mask: Enum);
    pub fn glFinish();
    pub fn glFlush();

    pub fn glGenBuffers(n: i32, out: *mut u32);
    pub fn glBindBuffer(target: Enum, buffer: u32);
    pub fn glBufferData(target: Enum, size: isize, data: *const c_void, usage: Enum);
    pub fn glBufferSubData(target: Enum, offset: isize, size: isize, data: *const c_void);
    pub fn glGenVertexArrays(n: i32, out: *mut u32);
    pub fn glBindVertexArray(array: u32);
    pub fn glEnableVertexAttribArray(index: u32);
    pub fn glVertexAttribPointer(index: u32, size: i32, kind: Enum, normalized: u8, stride: i32, pointer: *const c_void);

    pub fn glCreateShader(kind: Enum) -> u32;
    pub fn glShaderSource(shader: u32, count: i32, strings: *const *const c_char, lengths: *const i32);
    pub fn glCompileShader(shader: u32);
    pub fn glGetShaderiv(shader: u32, name: Enum, out: *mut i32);
    pub fn glGetShaderInfoLog(shader: u32, cap: i32, len: *mut i32, log: *mut c_char);
    pub fn glDeleteShader(shader: u32);
    pub fn glCreateProgram() -> u32;
    pub fn glAttachShader(program: u32, shader: u32);
    pub fn glBindAttribLocation(program: u32, index: u32, name: *const c_char);
    pub fn glLinkProgram(program: u32);
    pub fn glGetProgramiv(program: u32, name: Enum, out: *mut i32);
    pub fn glGetProgramInfoLog(program: u32, cap: i32, len: *mut i32, log: *mut c_char);
    pub fn glUseProgram(program: u32);
    pub fn glGetUniformLocation(program: u32, name: *const c_char) -> i32;
    pub fn glUniform1i(location: i32, v: i32);
    pub fn glUniform4fv(location: i32, count: i32, v: *const f32);
    pub fn glUniformMatrix4fv(location: i32, count: i32, transpose: u8, v: *const f32);

    pub fn glGenTextures(n: i32, out: *mut u32);
    pub fn glBindTexture(target: Enum, texture: u32);
    pub fn glActiveTexture(unit: Enum);
    pub fn glTexParameteri(target: Enum, name: Enum, value: i32);
    pub fn glTexParameterf(target: Enum, name: Enum, value: f32);
    pub fn glPixelStorei(name: Enum, value: i32);
    pub fn glTexStorage2D(target: Enum, levels: i32, format: Enum, w: i32, h: i32);
    pub fn glTexImage2D(target: Enum, level: i32, internal: i32, w: i32, h: i32, border: i32, format: Enum, kind: Enum, data: *const c_void);
    pub fn glTexSubImage2D(target: Enum, level: i32, x: i32, y: i32, w: i32, h: i32, format: Enum, kind: Enum, data: *const c_void);
    pub fn glCompressedTexImage2D(target: Enum, level: i32, format: Enum, w: i32, h: i32, border: i32, size: i32, data: *const c_void);
    pub fn glGenerateMipmap(target: Enum);

    pub fn glDrawElements(mode: Enum, count: i32, kind: Enum, offset: *const c_void);
    pub fn glDrawArrays(mode: Enum, first: i32, count: i32);

    pub fn glGenFramebuffers(n: i32, out: *mut u32);
    pub fn glBindFramebuffer(target: Enum, framebuffer: u32);
    pub fn glFramebufferTexture2D(target: Enum, attachment: Enum, textarget: Enum, texture: u32, level: i32);
    pub fn glCheckFramebufferStatus(target: Enum) -> Enum;
    pub fn glInvalidateFramebuffer(target: Enum, count: i32, attachments: *const Enum);
    pub fn glBlitFramebuffer(sx0: i32, sy0: i32, sx1: i32, sy1: i32, dx0: i32, dy0: i32, dx1: i32, dy1: i32, mask: Enum, filter: Enum);
}

/// A program of a vertex and a fragment source, with its attributes at the places `attributes` gives them.
pub unsafe fn program(name: &str, head: &str, vs: &str, fs: &str, attributes: &[&str]) -> Result<u32, String> {
    unsafe fn stage(name: &str, kind: Enum, head: &str, source: &str) -> Result<u32, String> {
        let shader = glCreateShader(kind);
        let parts = ["#version 300 es\n", head, source];
        let pointers = parts.map(|p| p.as_ptr() as *const c_char);
        let lengths = parts.map(|p| p.len() as i32);
        glShaderSource(shader, 3, pointers.as_ptr(), lengths.as_ptr());
        glCompileShader(shader);
        let mut ok = 0;
        glGetShaderiv(shader, COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut log = [0u8; 600];
            let mut len = 0;
            glGetShaderInfoLog(shader, log.len() as i32, &mut len, log.as_mut_ptr() as *mut c_char);
            return Err(format!("{name}: {}", String::from_utf8_lossy(&log[..len.clamp(0, 600) as usize])));
        }
        Ok(shader)
    }
    let (v, f) = (stage(&format!("{name} (vertex)"), VERTEX_SHADER, head, vs)?, stage(&format!("{name} (fragment)"), FRAGMENT_SHADER, head, fs)?);
    let p = glCreateProgram();
    glAttachShader(p, v);
    glAttachShader(p, f);
    for (i, a) in attributes.iter().enumerate() {
        let c = std::ffi::CString::new(*a).unwrap();
        glBindAttribLocation(p, i as u32, c.as_ptr());
    }
    glLinkProgram(p);
    glDeleteShader(v);
    glDeleteShader(f);
    let mut ok = 0;
    glGetProgramiv(p, LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut log = [0u8; 600];
        let mut len = 0;
        glGetProgramInfoLog(p, log.len() as i32, &mut len, log.as_mut_ptr() as *mut c_char);
        return Err(format!("{name}: {}", String::from_utf8_lossy(&log[..len.clamp(0, 600) as usize])));
    }
    Ok(p)
}

pub unsafe fn uniform(program: u32, name: &str) -> i32 {
    let c = std::ffi::CString::new(name).unwrap();
    glGetUniformLocation(program, c.as_ptr())
}

/// Says which texture unit each of a program's samplers reads.
pub unsafe fn samplers(program: u32, names: &[(&str, i32)]) {
    glUseProgram(program);
    for (name, unit) in names {
        let at = uniform(program, name);
        if at >= 0 {
            glUniform1i(at, *unit);
        }
    }
}

pub unsafe fn bind(unit: u32, texture: u32) {
    glActiveTexture(TEXTURE0 + unit);
    glBindTexture(TEXTURE_2D, texture);
}
