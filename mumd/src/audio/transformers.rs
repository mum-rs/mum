pub trait Transformer {
    fn transform<'a>(&mut self, buf: &'a mut [f32]) -> Option<&'a mut [f32]>;
}

pub struct NoiseGate {
    alltime_high: f32,
    open: usize,
    deactivation_delay: usize,
}

impl NoiseGate {
    pub fn new(deactivation_delay: usize) -> Self {
        Self {
            alltime_high: 0.0,
            open: 0,
            deactivation_delay,
        }
    }
}

impl Transformer for NoiseGate {
    fn transform<'a>(&mut self, buf: &'a mut [f32]) -> Option<&'a mut [f32]> {
        const MUTE_PERCENTAGE: f32 = 0.1;

        let max = buf.iter().map(|e| e.abs()).max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap();

        if max > self.alltime_high {
            self.alltime_high = max;
        }

        if max > self.alltime_high * MUTE_PERCENTAGE {
            self.open = self.deactivation_delay;
        } else if self.open > 0 {
            self.open -= 1;
        }

        if self.open == 0 {
            None
        } else {
            Some(buf)
        }
    }
}
