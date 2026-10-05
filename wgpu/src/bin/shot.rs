//! A frame of the city written to a file, on the build machine: the renderer
//! of the browser tab on this machine's GPU, reading the pack from disk.
//!
//!   tokyo-shot --pack city.pack --out frame.png [--shape ipod] [--size 480x320] [--samples 4]
//!              [--budget N] [--frames 240] [--status status.json] [--words "view=… hour=… near=… mid=…"]
//!              [--against other.png]
//!
//! The flight runs `--frames` frames of a sixtieth of a second and then on
//! until every cell near the eye has been read; the last frame is the picture.
//! `--words` are the development host's (`tokyo_sim::flight::Flight::control`):
//! `view=` holds the eye, `near=` and `mid=` hold the distances of the levels
//! of detail, `rate=0` stops the clock.
//!
//! `--against` names a picture of the same size (a device's capture of the
//! same view): the status then says how far the two are apart, as the mean
//! difference of a colour in 255ths and the share of pixels where a colour
//! differs by more than 16.

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    if let Err(e) = native::run() {
        eprintln!("tokyo-shot: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use pocket_web_wgpu::gpu::{Gpu, Screen};
    use pocket_web_wgpu::source::Source;
    use pocket_web_wgpu::task;
    use tokyo_core::Pad;
    use tokyo_wgpu::app::{App, Shape, SweepHere};

    fn option(name: &str) -> Option<String> {
        let args: Vec<String> = std::env::args().collect();
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
    }

    /// A PNG file as rows of RGBA.
    fn picture(path: &str) -> Result<Vec<u8>, String> {
        let mut decoder = png::Decoder::new(std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?);
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| format!("{path}: {e}"))?;
        let mut bytes = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut bytes).map_err(|e| format!("{path}: {e}"))?;
        bytes.truncate(info.buffer_size());
        Ok(match info.color_type {
            png::ColorType::Rgba => bytes,
            png::ColorType::Rgb => bytes.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            png::ColorType::Grayscale => bytes.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::GrayscaleAlpha => bytes.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            png::ColorType::Indexed => return Err(format!("{path}: a palette the decoder did not expand")),
        })
    }

    pub fn run() -> Result<(), String> {
        let pack = option("--pack").ok_or("--pack PATH")?;
        let out = option("--out").ok_or("--out PNG")?;
        let mut shape = Shape::named(&option("--shape").unwrap_or("ipod".into())).ok_or("--shape psp | vita | 3ds | ipod")?;
        if let Some(size) = option("--size") {
            let (w, h) = size.split_once('x').ok_or("--size WIDTHxHEIGHT")?;
            (shape.width, shape.height) = (w.parse().map_err(|_| "--size WIDTHxHEIGHT")?, h.parse().map_err(|_| "--size WIDTHxHEIGHT")?);
        }
        let number = |name: &str, fallback: u32| option(name).map_or(Ok(fallback), |v| v.parse::<u32>().map_err(|_| format!("{name} takes a number")));
        shape.samples = number("--samples", shape.samples)?;
        shape.budget = number("--budget", shape.budget)?;
        let frames = number("--frames", 240)?;

        let gpu = task::wait(Gpu::headless())?;
        let screen = Screen::texture(&gpu, shape.width, shape.height, shape.samples);
        let mut app = task::wait(App::start(gpu, screen, shape, Source::new(&pack), |tables| Box::new(SweepHere::new(tables))))?;
        if let Some(words) = option("--words") {
            app.control(&words);
        }
        let pad = Pad { buttons: 0, keys: 0, lx: 0.0, ly: 0.0, rx: 0.0, ry: 0.0 };
        let step = 1000.0 / 60.0 * shape.pace() as f64;
        let mut count = 0;
        // (a few frames after the last cell arrived: the frame that drew it has been chosen with it ready)
        let mut quiet = 0;
        while count < frames || quiet < 4 {
            app.frame(count as f64 * step, &pad)?;
            count += 1;
            quiet = if app.settled() { quiet + 1 } else { 0 };
            if count > frames + 2000 {
                return Err("the cells near the eye did not all arrive".into());
            }
        }
        let pixels = task::wait(app.screen.read(&app.gpu))?;
        let file = std::fs::File::create(&out).map_err(|e| format!("{out}: {e}"))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), shape.width, shape.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut w| w.write_image_data(&pixels)).map_err(|e| format!("{out}: {e}"))?;
        let mut status = app.status();
        if let Some(path) = option("--against") {
            let other = picture(&path)?;
            if other.len() != pixels.len() {
                return Err(format!("{path} is not {} by {} pixels", shape.width, shape.height));
            }
            let (mut sum, mut far) = (0u64, 0u32);
            for (a, b) in pixels.as_chunks::<4>().0.iter().zip(other.as_chunks::<4>().0) {
                let d = [0, 1, 2].map(|c| a[c].abs_diff(b[c]));
                sum += d.iter().map(|&d| d as u64).sum::<u64>();
                far += d.iter().any(|&d| d > 16) as u32;
            }
            let count = (pixels.len() / 4) as f64;
            status.pop();
            status.push_str(&format!(",\"against\":{{\"picture\":\"{path}\",\"mean\":{:.3},\"over16\":{:.4}}}}}", sum as f64 / (count * 3.0), far as f64 / count));
        }
        if let Some(path) = option("--status") {
            std::fs::write(&path, &status).map_err(|e| format!("{path}: {e}"))?;
        }
        println!("{status}");
        Ok(())
    }
}
