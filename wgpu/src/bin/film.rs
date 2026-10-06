//! A flight filmed on the build machine: the renderer of the browser tab on
//! this machine's GPU, one frame for each line of a list, written to standard
//! output as raw RGBA. `tools/listing.ts` writes the list and hands the frames
//! to ffmpeg.
//!
//!   tokyo-film --pack city.pack --frames LIST [--shape vita] [--size 960x544] [--samples 4]
//!              [--budget N] [--ticks 1] > frames.rgba
//!
//! `--pack` names the pack's file, or the manifest (`.json`) of a pack cut into
//! pieces. A line of `LIST` is one frame: the development host's words for it
//! (`tokyo_sim::flight::Flight::control`), or nothing. `view=` holds the eye,
//! `hour=` sets the clock, `rate=` its speed, `tour=1 at=40` puts the eye on
//! the tour, 40 seconds along it. Words stay as they were set until a later
//! line changes them, so a tour or a clock runs by itself from its first line.
//!
//! Each frame the flight advances `--ticks` sixtieths of a second (1 is 60
//! frames a second, 2 is 30), and the frame is drawn again until every cell
//! near the eye and every block's picture has arrived, so no frame shows a
//! cell on its way. The first line's words are applied before that wait too.
//! The reads are from a file and the sweep of the shadows is the frame's own:
//! the same list gives the same film.

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    if let Err(e) = native::run() {
        eprintln!("tokyo-film: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::Write;

    use pocket_web_wgpu::gpu::{Gpu, Screen};
    use pocket_web_wgpu::source::Source;
    use pocket_web_wgpu::task;
    use tokyo_wgpu::app::{App, City, Held, Shape, SweepHere};

    fn option(name: &str) -> Option<String> {
        let args: Vec<String> = std::env::args().collect();
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
    }

    /// Draws the frame again until what the eye is near has arrived.
    fn settle(app: &mut App) -> Result<(), String> {
        let (mut quiet, mut tries) = (0, 0);
        while quiet < 2 {
            app.draw()?;
            quiet = if app.settled() { quiet + 1 } else { 0 };
            tries += 1;
            if tries > 2000 {
                return Err("the cells near the eye did not all arrive".into());
            }
        }
        Ok(())
    }

    pub fn run() -> Result<(), String> {
        let pack = option("--pack").ok_or("--pack PATH")?;
        let list = option("--frames").ok_or("--frames LIST")?;
        let list = std::fs::read_to_string(&list).map_err(|e| format!("{list}: {e}"))?;
        let mut shape = Shape::named(&option("--shape").unwrap_or("vita".into())).ok_or("--shape psp | vita | 3ds | ipod")?;
        if let Some(size) = option("--size") {
            let (w, h) = size.split_once('x').ok_or("--size WIDTHxHEIGHT")?;
            (shape.width, shape.height) = (w.parse().map_err(|_| "--size WIDTHxHEIGHT")?, h.parse().map_err(|_| "--size WIDTHxHEIGHT")?);
        }
        let number = |name: &str, fallback: u32| option(name).map_or(Ok(fallback), |v| v.parse::<u32>().map_err(|_| format!("{name} takes a number")));
        shape.samples = number("--samples", shape.samples)?;
        shape.budget = number("--budget", shape.budget)?;
        let ticks = number("--ticks", 1)?.clamp(1, 3);

        let gpu = task::wait(Gpu::headless())?;
        let screen = Screen::texture(&gpu, shape.width, shape.height, shape.samples);
        let source = task::wait(Source::open(&pack))?;
        let city = task::wait(City::read(gpu.clone(), screen.format, shape.samples, source))?;
        let mut app = App::open(gpu, screen, shape);
        app.fly(city, |tables| Box::new(SweepHere::new(tables)))?;

        let pad = Held::default();
        let lines: Vec<&str> = list.lines().map(str::trim).filter(|line| !line.starts_with('#')).collect();
        let mut out = std::io::BufWriter::with_capacity(1 << 22, std::io::stdout().lock());
        let mut now = 0.0f64;
        // The city where the film starts, read before its first frame.
        if let Some(first) = lines.first() {
            app.control(first);
            app.frame(now, &pad)?;
            settle(&mut app)?;
        }
        for line in &lines {
            if !line.is_empty() {
                app.control(line);
            }
            now += ticks as f64 * 1000.0 / 60.0;
            app.step(now, &pad);
            settle(&mut app)?;
            let pixels = task::wait(app.screen.read(&app.gpu))?;
            out.write_all(&pixels).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        eprintln!("{}", app.status());
        Ok(())
    }
}
