use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

/// 混成立体声为单声道 + 线性插值重采样到 16kHz
pub fn resample_to_16k_mono(samples: &[f32], src_rate: u32, channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = samples
        .chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    if src_rate == 16_000 {
        return mono;
    }
    let ratio = src_rate as f64 / 16_000.0;
    let out_len = (mono.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = mono[idx.min(mono.len() - 1)];
            let b = mono[(idx + 1).min(mono.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// 打开默认输入设备，回调把原始 f32 帧推入 channel
pub fn start_capture() -> Result<AudioCapture, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or("找不到可用的麦克风设备")?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("读取麦克风配置失败: {e}"))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let err_fn = |e| eprintln!("audio capture error: {e}");

    let stream = match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let _ = tx.send(data.to_vec());
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("打开麦克风失败（请检查系统录音权限）: {e}"))?,
        SampleFormat::I16 => device
            .build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let converted: Vec<f32> = data
                        .iter()
                        .map(|&s| s as f32 / i16::MAX as f32)
                        .collect();
                    let _ = tx.send(converted);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("打开麦克风失败（请检查系统录音权限）: {e}"))?,
        SampleFormat::U16 => device
            .build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let converted: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 - 32768.0) / 32768.0)
                        .collect();
                    let _ = tx.send(converted);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("打开麦克风失败（请检查系统录音权限）: {e}"))?,
        _ => {
            return Err(format!(
                "不支持的麦克风采样格式: {:?}",
                sample_format
            ))
        }
    };

    stream.play().map_err(|e| format!("启动音频流失败: {e}"))?;
    Ok(AudioCapture {
        stream,
        rx,
        sample_rate,
        channels,
    })
}

pub struct AudioCapture {
    pub stream: cpal::Stream,
    pub rx: std::sync::mpsc::Receiver<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_to_mono_averages_channels() {
        // 左右声道各一个采样对
        let mono = resample_to_16k_mono(&[1.0, 0.0, 0.5, 0.5], 16_000, 2);
        assert_eq!(mono, vec![0.5, 0.5]);
    }

    #[test]
    fn downsamples_48k_to_16k_by_third() {
        let input: Vec<f32> = (0..480).map(|i| i as f32).collect();
        let out = resample_to_16k_mono(&input, 48_000, 1);
        assert_eq!(out.len(), 160);
        assert_eq!(out[0], 0.0);
    }

    #[test]
    fn passthrough_when_already_16k_mono() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_to_16k_mono(&input, 16_000, 1), input);
    }
}
