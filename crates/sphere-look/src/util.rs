//! Small shared helpers: the prototype's spring, colour roles (Balanced theme) and clamps.
use bevy::prelude::*;

pub const TAU: f32 = std::f32::consts::TAU;
pub const DEG: f32 = std::f32::consts::PI / 180.0;

pub fn clamp(x: f32, a: f32, b: f32) -> f32 {
    x.max(a).min(b)
}
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
pub fn out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - clamp(t, 0.0, 1.0)).powi(3)
}

/// Damped spring, a straight port of the prototype's `Spring` (sub-stepped so stiff springs stay stable).
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub x: f32,
    pub t: f32,
    pub v: f32,
    pub k: f32,
    pub c: f32,
}
impl Spring {
    pub fn new(x: f32, k: f32, c: f32) -> Self {
        Self { x, t: x, v: 0.0, k, c }
    }
    pub fn step(&mut self, dt: f32) -> f32 {
        let n = ((dt * 120.0).ceil() as i32).max(1);
        let h = dt / n as f32;
        for _ in 0..n {
            let a = self.k * (self.t - self.x) - self.c * self.v;
            self.v += a * h;
            self.x += self.v * h;
        }
        if !self.x.is_finite() {
            self.set(self.t);
        }
        self.x
    }
    pub fn set(&mut self, x: f32) {
        self.x = x;
        self.t = x;
        self.v = 0.0;
    }
}

/// The Balanced theme: cool blue-white lines, colour only for meaning, amber for "needs you".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    Line,
    Dim,
    Hot,
    Fill,
    Acc,
    Deep,
    Ok,
    #[allow(dead_code)]
    Bad,
}

#[allow(dead_code)]
pub const BLOOM: f32 = 0.6;

pub fn hex(s: &str) -> Color {
    Color::srgb_u8(
        u8::from_str_radix(&s[1..3], 16).unwrap(),
        u8::from_str_radix(&s[3..5], 16).unwrap(),
        u8::from_str_radix(&s[5..7], 16).unwrap(),
    )
}

pub fn role_hex(r: Role) -> &'static str {
    match r {
        Role::Line => "#a6d3e6",
        Role::Dim => "#3e6678",
        Role::Hot => "#f1f9fc",
        Role::Fill => "#dff4ff",
        Role::Acc => "#ffb347",
        Role::Deep => "#2f7493",
        Role::Ok => "#5fe3a1",
        Role::Bad => "#ff5a4f",
    }
}
pub fn role_color(r: Role) -> Color {
    hex(role_hex(r))
}
pub fn role_lin(r: Role) -> Vec4 {
    let c = role_color(r).to_linear();
    Vec4::new(c.red, c.green, c.blue, 1.0)
}
