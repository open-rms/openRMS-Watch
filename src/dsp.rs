//! Streaming audio frequency shifter: move a real 8 kHz audio signal up or down
//! by a few tens of Hz (SSB-style, no image) to correct an off-frequency receiver.
//!
//! analytic = delayed(x) + j * hilbert(x);  y = Re(analytic * e^{j w n})

const TAPS: usize = 255; // odd; ~31 Hz transition at 8 kHz, plenty for 300-2700 Hz audio

pub struct Shifter {
    h: Vec<f32>,
    hist: Vec<f32>,
    pos: usize,
    phase: f64,
    dphi: f64,
}

impl Shifter {
    pub fn new(shift_hz: f64, rate: f64) -> Option<Shifter> {
        if shift_hz == 0.0 {
            return None;
        }
        let m = (TAPS / 2) as i64;
        let h = (0..TAPS)
            .map(|i| {
                let k = i as i64 - m;
                if k % 2 == 0 {
                    0.0
                } else {
                    let w = 0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (TAPS - 1) as f64).cos();
                    (2.0 / (std::f64::consts::PI * k as f64) * w) as f32
                }
            })
            .collect();
        Some(Shifter {
            h,
            hist: vec![0.0; TAPS],
            pos: 0,
            phase: 0.0,
            dphi: 2.0 * std::f64::consts::PI * shift_hz / rate,
        })
    }

    pub fn process(&mut self, buf: &mut [i16]) {
        for s in buf.iter_mut() {
            self.hist[self.pos] = *s as f32;
            // newest sample at pos; hist[pos - i] is x[n - i]
            let mut q = 0.0f32;
            // non-zero taps are where (i - TAPS/2) is odd
            for i in ((TAPS / 2 + 1) % 2..TAPS).step_by(2) {
                let j = (self.pos + TAPS - i) % TAPS;
                q += self.h[i] * self.hist[j];
            }
            // Delay the real part by the filter's group delay (TAPS/2).
            // Filter taps h[i] multiply x[n-i]; centre tap index TAPS/2.
            let idel = self.hist[(self.pos + TAPS - TAPS / 2) % TAPS];
            // h[i] = 2/(pi k), k = i - m, convolved as sum h[i] x[n-i]: this
            // is +hilbert(x) delayed by m (checked by the tests below).
            let (sn, cs) = self.phase.sin_cos();
            let y = idel as f64 * cs - q as f64 * sn;
            *s = y.clamp(-32768.0, 32767.0) as i16;
            self.phase = (self.phase + self.dphi) % (2.0 * std::f64::consts::PI);
            self.pos = (self.pos + 1) % TAPS;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_power(x: &[i16], f: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &s) in x.iter().enumerate() {
            let a = 2.0 * std::f64::consts::PI * f * n as f64 / 8000.0;
            re += s as f64 * a.cos();
            im -= s as f64 * a.sin();
        }
        (re * re + im * im).sqrt() / x.len() as f64
    }

    #[test]
    fn shifts_up_without_image() {
        let mut x: Vec<i16> = (0..16000)
            .map(|n| (8000.0 * (2.0 * std::f64::consts::PI * 1500.0 * n as f64 / 8000.0).sin()) as i16)
            .collect();
        Shifter::new(100.0, 8000.0).unwrap().process(&mut x);
        let x = &x[1000..];
        let want = tone_power(x, 1600.0);
        let image = tone_power(x, 1400.0);
        let orig = tone_power(x, 1500.0);
        assert!(want > 3500.0, "shifted tone too weak: {want}");
        assert!(image < want / 100.0, "image {image} vs {want}");
        assert!(orig < want / 100.0, "residual {orig} vs {want}");
    }

    #[test]
    fn shifts_down() {
        let mut x: Vec<i16> = (0..16000)
            .map(|n| (8000.0 * (2.0 * std::f64::consts::PI * 1500.0 * n as f64 / 8000.0).sin()) as i16)
            .collect();
        Shifter::new(-60.0, 8000.0).unwrap().process(&mut x);
        let x = &x[1000..];
        assert!(tone_power(x, 1440.0) > 100.0 * tone_power(x, 1560.0));
    }
}
