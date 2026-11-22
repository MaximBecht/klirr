//! Reading impulse responses from WAV files.

use std::error::Error;

/// Reads the first channel of a WAV file: the samples (as f32) and the sample rate.
///
/// # Errors
///
/// If the file cannot be read or is not a valid WAV file.
pub fn read_mono(path: &str) -> Result<(Vec<f32>, u32), Box<dyn Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    Ok((
        interleaved.chunks(channels).map(|f| f[0]).collect(),
        spec.sample_rate,
    ))
}
