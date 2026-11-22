//! Uniformly partitioned FFT convolution (overlap-save), used for cabinet impulse responses.
//!
//! The impulse response is cut into partitions of [`BLOCK`] samples. Every [`BLOCK`] input
//! samples, one FFT of the newest input frame is multiplied with all partition spectra and
//! summed, followed by one inverse FFT. Cost grows linearly with the IR length, and the work
//! happens on a fixed grid, so the output never depends on how the host splits its blocks.
//! Latency is exactly [`BLOCK`] samples.

use std::fmt;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// Partition size and latency, in samples.
pub const BLOCK: usize = 32;
const FFT_LEN: usize = 2 * BLOCK;
const BINS: usize = BLOCK + 1;

/// A running convolution with a fixed impulse response.
pub struct Convolver {
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    ir: Vec<Complex<f32>>,
    fdl: Vec<Complex<f32>>,
    partitions: usize,
    fdl_pos: usize,
    input: [f32; FFT_LEN],
    output: [f32; BLOCK],
    pos: usize,
    time: Vec<f32>,
    acc: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl fmt::Debug for Convolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Convolver")
            .field("partitions", &self.partitions)
            .finish_non_exhaustive()
    }
}

impl Convolver {
    /// Prepares a convolver for `impulse_response`. Allocates; call outside the audio callback.
    ///
    /// An empty impulse response produces silence.
    pub fn new(impulse_response: &[f32]) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(FFT_LEN);
        let c2r = planner.plan_fft_inverse(FFT_LEN);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());
        let partitions = impulse_response.len().div_ceil(BLOCK);

        let mut ir = vec![Complex::default(); partitions * BINS];
        let mut time = vec![0.0; FFT_LEN];
        let mut scratch = vec![Complex::default(); scratch_len];
        for (k, chunk) in impulse_response.chunks(BLOCK).enumerate() {
            time.fill(0.0);
            time[..chunk.len()].copy_from_slice(chunk);
            r2c.process_with_scratch(&mut time, &mut ir[k * BINS..(k + 1) * BINS], &mut scratch)
                .expect("FFT buffer sizes are fixed");
        }

        Self {
            r2c,
            c2r,
            ir,
            fdl: vec![Complex::default(); partitions * BINS],
            partitions,
            fdl_pos: 0,
            input: [0.0; FFT_LEN],
            output: [0.0; BLOCK],
            pos: 0,
            time,
            acc: vec![Complex::default(); BINS],
            scratch,
        }
    }

    /// Clears the running state (keeps the impulse response).
    pub fn reset(&mut self) {
        self.fdl.fill(Complex::default());
        self.input = [0.0; FFT_LEN];
        self.output = [0.0; BLOCK];
        self.pos = 0;
        self.fdl_pos = 0;
    }

    /// Feeds one sample and returns the output from [`BLOCK`] samples ago.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.output[self.pos];
        self.input[BLOCK + self.pos] = x;
        self.pos += 1;
        if self.pos == BLOCK {
            self.pos = 0;
            self.process_block();
        }
        y
    }

    fn process_block(&mut self) {
        if self.partitions == 0 {
            self.output = [0.0; BLOCK];
            return;
        }
        let p = self.partitions;
        self.time.copy_from_slice(&self.input);
        let slot = self.fdl_pos * BINS;
        self.r2c
            .process_with_scratch(
                &mut self.time,
                &mut self.fdl[slot..slot + BINS],
                &mut self.scratch,
            )
            .expect("FFT buffer sizes are fixed");

        self.acc.fill(Complex::default());
        for k in 0..p {
            let s = ((self.fdl_pos + p - k) % p) * BINS;
            let x = &self.fdl[s..s + BINS];
            let h = &self.ir[k * BINS..(k + 1) * BINS];
            for ((a, x), h) in self.acc.iter_mut().zip(x).zip(h) {
                *a += x * h;
            }
        }
        // Real input spectra: DC and Nyquist bins are purely real.
        self.acc[0].im = 0.0;
        self.acc[BINS - 1].im = 0.0;
        self.c2r
            .process_with_scratch(&mut self.acc, &mut self.time, &mut self.scratch)
            .expect("FFT buffer sizes are fixed");

        let scale = 1.0 / FFT_LEN as f32;
        for (o, t) in self.output.iter_mut().zip(&self.time[BLOCK..]) {
            *o = t * scale;
        }
        self.input.copy_within(BLOCK.., 0);
        self.fdl_pos = (self.fdl_pos + 1) % p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo_random(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn matches_direct_convolution() {
        let ir = pseudo_random(300, 1);
        let x = pseudo_random(1000, 2);
        let mut conv = Convolver::new(&ir);
        let y: Vec<f32> = x.iter().map(|&v| conv.process(v)).collect();
        for (n, &got) in y.iter().enumerate().skip(BLOCK) {
            let t = n - BLOCK;
            let expected: f32 = (0..ir.len())
                .filter(|&k| k <= t)
                .map(|k| ir[k] * x[t - k])
                .sum();
            assert!(
                (got - expected).abs() < 1e-4,
                "n = {n}: {got} vs {expected}"
            );
        }
    }

    #[test]
    fn empty_ir_is_silent() {
        let mut conv = Convolver::new(&[]);
        for _ in 0..100 {
            assert_eq!(conv.process(1.0), 0.0);
        }
    }
}
