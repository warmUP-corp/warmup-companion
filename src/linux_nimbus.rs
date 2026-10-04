use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const RT: u32 = 72;
const STEPS: i32 = 36;
const LIGHT_STEPS: i32 = 3;
const DENSITY_OCT: i32 = 4;
const WHITE_LIFT: f32 = 0.35;

struct Params {
    input: f32,
    output: f32,
    speed: f32,
    cam_dist: f32,
    focal: f32,
    radius: f32,
    scale: f32,
    churn: f32,
    threshold: f32,
    edge_soft: f32,
    density: f32,
    absorb: f32,
    shadow_absorb: f32,
    shadow_lift: f32,
    aniso: f32,
    light_spin: f32,
    power: f32,
    ambient: f32,
    exposure: f32,
    alpha_gain: f32,
    swirl: f32,
    light: [f32; 3],
    shadow: [f32; 3],
}

struct Animator {
    noise: f32,
    light: f32,
    swirl: f32,
    age: f32,
    last: Option<Instant>,
    frame: Vec<u8>,
}

static LATEST: OnceLock<Mutex<Option<(u32, Vec<u8>)>>> = OnceLock::new();
static ANIMATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn ensure_running() {
    if ANIMATING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-nimbus".into())
        .spawn(|| {
            let mut anim = Animator::new();
            while crate::linux_speech::visual_active() {
                let phase = crate::linux_speech::phase();
                let level = crate::linux_speech::level();
                let frame = anim.render(phase, level, [0.65, 0.82, 1.0]);
                if let Ok(mut slot) = frame_slot().lock() {
                    *slot = Some((RT, frame));
                }
                std::thread::sleep(std::time::Duration::from_millis(33));
            }
            if let Ok(mut slot) = frame_slot().lock() {
                *slot = None;
            }
            ANIMATING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
}

pub fn latest_frame() -> Option<(u32, Vec<u8>)> {
    frame_slot().lock().ok().and_then(|guard| guard.clone())
}

fn frame_slot() -> &'static Mutex<Option<(u32, Vec<u8>)>> {
    LATEST.get_or_init(|| Mutex::new(None))
}

impl Animator {
    fn new() -> Self {
        Self {
            noise: 0.0,
            light: 0.0,
            swirl: 0.0,
            age: 0.0,
            last: None,
            frame: vec![0; (RT * RT * 4) as usize],
        }
    }

    fn render(&mut self, phase: u8, level: f32, accent: [f32; 3]) -> Vec<u8> {
        let now = Instant::now();
        let dt = self
            .last
            .map(|t| now.saturating_duration_since(t).as_secs_f32().min(0.05))
            .unwrap_or(1.0 / 30.0);
        self.last = Some(now);
        self.age += dt;
        let level = level.clamp(0.0, 1.0);
        let pulse = match phase {
            2 => {
                let think = 0.5 + 0.5 * (self.age * 2.4).sin();
                think.clamp(0.0, 1.0)
            }
            1 => 0.5 + 0.5 * level,
            _ => 0.5,
        };
        let noise_rate = match phase {
            1 => 0.22 + 0.2 * level,
            2 => 0.4 + 0.22 * pulse,
            _ => 0.22,
        };
        self.noise += dt * noise_rate;
        self.light += dt * 0.08;
        let swirl_rate = match phase {
            1 => 0.15 + 0.4 * level,
            2 => 0.30,
            _ => 0.15,
        };
        self.swirl = (self.swirl + dt * swirl_rate).rem_euclid(std::f32::consts::TAU);
        let glow = pulse;
        let params = Params {
            input: 0.55,
            output: pulse,
            speed: self.noise,
            cam_dist: 4.4,
            focal: 1.8,
            radius: 2.0,
            scale: 1.15,
            churn: glow,
            threshold: 0.40 - 0.16 * glow,
            edge_soft: 0.5,
            density: 1.35 + 2.6 * glow,
            absorb: 0.72,
            shadow_absorb: 1.5,
            shadow_lift: 0.7,
            aniso: 0.4,
            light_spin: self.light,
            power: 0.8 + 0.45 * glow,
            ambient: 0.06 + 0.08 * glow,
            exposure: 0.50 + 0.20 * glow,
            alpha_gain: 1.0,
            swirl: self.swirl,
            light: accent,
            shadow: [accent[0] * 0.7, accent[1] * 0.7, accent[2] * 0.7],
        };
        render_into(&params, &mut self.frame);
        self.frame.clone()
    }
}

fn render_into(p: &Params, out: &mut [u8]) {
    let res = RT as f32;
    for y in 0..RT {
        for x in 0..RT {
            let (r, g, b, a) = pixel(p, x as f32 + 0.5, y as f32 + 0.5, res);
            let i = ((y * RT + x) * 4) as usize;
            let pa = a.clamp(0.0, 1.0);
            out[i] = (r.clamp(0.0, 1.0) * pa * 255.0) as u8;
            out[i + 1] = (g.clamp(0.0, 1.0) * pa * 255.0) as u8;
            out[i + 2] = (b.clamp(0.0, 1.0) * pa * 255.0) as u8;
            out[i + 3] = (pa * 255.0) as u8;
        }
    }
}

fn pixel(p: &Params, fx: f32, fy: f32, res: f32) -> (f32, f32, f32, f32) {
    let fy = res - fy;
    let uvx = (2.0 * fx - res) / res;
    let uvy = (2.0 * fy - res) / res;
    let ro = [0.0, 0.0, -p.cam_dist];
    let rd = normalize([uvx, uvy, p.focal]);
    let light_dir = normalize([
        p.light_spin.cos() * 0.7,
        0.45,
        p.light_spin.sin() * 0.35 + 0.65,
    ]);
    let phase = phase_hg(dot(rd, light_dir), p.aniso);
    let ray_dist = length(cross(rd, ro));
    if ray_dist > p.radius + 0.08 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let t_start = (p.cam_dist - p.radius).max(0.0);
    let dt = (2.0 * p.radius) / STEPS as f32;
    let mut transmit = 1.0f32;
    let mut scattered = [0.0f32; 3];
    let nimbus_power = p.power;
    let nimbus_density = p.density * (1.0 + 0.35 * p.input);
    for i in 0..STEPS {
        let t = t_start + (i as f32 + 0.5) * dt;
        let pos = add(ro, mul(rd, t));
        let dn = density(p, pos, nimbus_density);
        if dn > 0.001 {
            let mut shadow = 1.0f32;
            let lstep = p.radius / LIGHT_STEPS as f32;
            for s in 1..=LIGHT_STEPS {
                let lp = add(pos, mul(light_dir, (s as f32 - 0.5) * lstep));
                shadow *= (-density(p, lp, nimbus_density) * lstep * p.shadow_absorb).exp();
            }
            let hue = 0.6 * (dot(pos, [0.8, 0.5, -0.3]) * 1.4 + p.swirl).sin();
            let light = hue_rotate(p.light, hue);
            let lit = lerp3(mul(p.shadow, p.shadow_lift), light, shadow);
            let gain = transmit * dn * dt * phase * nimbus_power;
            scattered = add(scattered, mul(lit, gain));
            transmit *= (-dn * dt * p.absorb).exp();
            if transmit < 0.01 {
                break;
            }
        }
    }
    let body = 1.0 - transmit;
    scattered = add(scattered, mul(p.light, body * p.ambient));
    let cover = smoothstep(p.radius + 0.05, p.radius - 0.14, ray_dist);
    let mut rgb = add(mul(p.shadow, cover), scattered);
    rgb = mul(rgb, p.exposure);
    let peak = rgb[0].max(rgb[1]).max(rgb[2]).max(0.0001);
    let tone = tanh_f(peak);
    let hue = mul(rgb, tone / peak);
    let lifted = [
        tanh_f(rgb[0]),
        tanh_f(rgb[1]),
        tanh_f(rgb[2]),
    ];
    let col = lerp3(hue, lifted, WHITE_LIFT * saturate(p.churn));
    let alpha = saturate(cover * p.alpha_gain);
    (col[0], col[1], col[2], alpha)
}

fn density(p: &Params, pos: [f32; 3], nimbus_density: f32) -> f32 {
    let shell = 1.0 - length(pos) / p.radius;
    if shell <= 0.0 {
        return 0.0;
    }
    let voice = saturate(p.churn);
    let pulse = saturate(p.output);
    let amp = 0.55 + 0.3 * (p.speed * 2.2).sin() + 0.5 * voice + 0.95 * pulse;
    let axis = normalize([0.35 * p.swirl.sin(), 1.0, 0.35 * p.swirl.cos()]);
    let mut swirled = rotate_axis(pos, axis, p.swirl);
    swirled = add(
        swirled,
        mul(
            [
                (p.swirl + 1.7).sin(),
                (p.swirl * 2.0 + 0.4).cos(),
                (p.swirl + 2.9).sin(),
            ],
            0.05,
        ),
    );
    let mut q = mul(swirled, p.scale * (1.0 + 0.16 * voice + 0.5 * pulse));
    let mut f = 1.0f32;
    for _ in 0..DENSITY_OCT {
        q = add(
            q,
            mul(
                [
                    (q[1] * f + p.speed).cos(),
                    (q[2] * f + p.speed).cos(),
                    (q[0] * f + p.speed).cos(),
                ],
                amp / f,
            ),
        );
        f *= 1.8;
    }
    let n = (q[0].sin() + q[1].sin() + q[2].sin()) / 3.0 * 0.5 + 0.5;
    let thresh = lerp(p.threshold, p.threshold * 0.45, pulse);
    let clump = smoothstep(thresh, 1.0, n);
    clump * shell.powf(p.edge_soft) * nimbus_density
}

fn phase_hg(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    (1.0 - g2) / (1.0 + g2 - 2.0 * g * c).max(0.0001).powf(1.5)
}

fn rotate_axis(pos: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let s = angle.sin();
    let c = angle.cos();
    add(
        add(mul(pos, c), mul(cross(axis, pos), s)),
        mul(axis, dot(axis, pos) * (1.0 - c)),
    )
}

fn hue_rotate(color: [f32; 3], angle: f32) -> [f32; 3] {
    let k = [0.57735, 0.57735, 0.57735];
    let ca = angle.cos();
    add(
        add(mul(color, ca), mul(cross(k, color), angle.sin())),
        mul(k, dot(k, color) * (1.0 - ca)),
    )
}

fn tanh_f(x: f32) -> f32 {
    let x = x.clamp(-10.0, 10.0);
    let e = (2.0 * x).exp();
    (e - 1.0) / (e + 1.0)
}

fn saturate(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = saturate((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

fn length(p: [f32; 3]) -> f32 {
    (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt()
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn mul(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn normalize(p: [f32; 3]) -> [f32; 3] {
    let len = length(p).max(1.0e-6);
    mul(p, 1.0 / len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_frame_has_a_cloud() {
        let mut anim = Animator::new();
        let started = Instant::now();
        let frame = anim.render(1, 0.6, [0.65, 0.82, 1.0]);
        let elapsed = started.elapsed();
        let lit = frame.chunks_exact(4).filter(|px| px[3] > 8).count();
        assert!(lit > 40, "lit pixels {lit}");
        assert!(elapsed.as_millis() < 800, "nimbus frame took {elapsed:?}");
    }
}
