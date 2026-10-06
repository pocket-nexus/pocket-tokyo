// tools/listing.ts: Pocket Tokyo's listing on Pocket Studio.
//
//   bun tools/listing.ts                  film every clip and still → dist/listing/
//   bun tools/listing.ts --only NAME      one file of it (tower.mp4, zojoji.jpg, card.jpg, …)
//   bun tools/listing.ts --upload         then `pocket-studio listing dist/listing`
//
// A listing is what a game's page on Pocket Studio shows: one sentence, a few
// paragraphs, clips and stills, and the picture a link preview carries. The
// words are `listing/listing.json`, in Git. The pictures are filmed here from
// the game: the renderer of the browser tab (`wgpu/`, the iPod touch's passes
// from the iPod touch's pack) on this machine's GPU at the PS Vita's screen
// size, one frame for each line of a list (`wgpu/src/bin/film.rs`), encoded by
// ffmpeg. The flight's clock, its tour and its traffic advance two sixtieths
// of a second a frame, and a frame is drawn when every cell near the eye has
// been read: a run gives the pictures the run before it gave. No picture goes
// to Git: `dist/` is ignored.
//
// A clip is the game's own tour from a second of its route, or an eye carried
// along a row of points, with the clock at an hour and running at a rate. A
// clip fades from black and to black over a third of a second, so it loops
// through a dip. A still is one held view at an hour.
//
// It needs the iPod touch's pack (`bun tools/wgpu.ts cook`), ffmpeg and
// ffprobe. `--upload` runs from the repository's root, where `pocket-studio
// register` wrote `.pocket-studio.json`; POCKET_STUDIO_CLI names the command
// when it is not on PATH.

import { existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
const CRATE = join(ROOT, "wgpu");
const WORK = join(ROOT, ".pocket-build/listing");
const OUT = join(ROOT, "dist/listing");
const FILM = join(CRATE, "target/release/tokyo-film");
const PACK = join(ROOT, ".pocket-build/city/shiba/ipod60/city.pack");

/** The PS Vita's screen, which the browser tab draws at; a clip's frames a second. */
const WIDTH = 960, HEIGHT = 544, RATE = 30;
/** Frames a clip fades over at each end. */
const FADE = 10;
const MIB = 1 << 20;
/** What Pocket Studio takes (the listing contract): a clip, a picture, the share picture, the whole directory. */
const LIMIT = { video: 12 * MIB, image: 2 * MIB, card: 1 * MIB, total: 96 * MIB };

/** An eye and the point it looks at, in the city's metres: east, up, south of Tokyo Tower's foot. */
type Point = [number, number, number, number, number, number];

interface Clip {
  seconds: number;
  /** The clock where the clip starts, and the hours it runs over the clip. */
  hour: number;
  hours: number;
  /** The game's tour from this second of its route, or an eye carried through these points. */
  tour?: number;
  path?: Point[];
  /** The second of the clip its poster shows. */
  poster: number;
}
interface Still {
  view: Point;
  field?: number;
  hour: number;
}

// The flight's day is in early October (`DAY` in n3ds/core/src/lib.rs): the sun rises at 6:15 and sets at
// 17:45. The lamps and the rooms come on over the half hour around sunset and go out over the half hour
// around sunrise (`night` in crates/tokyo-sim/src/sky.rs).
const CLIPS: Record<string, Clip> = {
  // The tour comes in from the south-east and goes round the tower while the evening falls.
  "tower.mp4": { seconds: 22, hour: 17.05, hours: 1.75, tour: 15, poster: 11 },
  // From Azabudai past the tower and over Zojo-ji, then east above the avenue from the temple's gate through
  // Daimon to the railway at Hamamatsucho, Kyu-Shiba-rikyu Garden and the bay, in the afternoon. The eye
  // stays over open ground and the avenue: a handheld keeps it out of the buildings, and so does this.
  "bay.mp4": {
    seconds: 18, hour: 15.5, hours: 0.45, poster: 2.7,
    path: [
      [-700, 260, 120, 0, 150, 60],
      [-330, 225, 190, 350, 60, 180],
      [40, 185, 205, 650, 30, 190],
      [400, 135, 185, 1000, 20, 185],
      [760, 125, 183, 1300, 15, 230],
      [1060, 142, 210, 1320, 0, 470],
      [1150, 132, 270, 1330, 0, 520],
    ],
  },
  // One view to the east while the night ends: the lamps go out and the sun comes up behind the tower.
  "sunrise.mp4": {
    seconds: 12, hour: 5 + 1 / 3, hours: 8 / 3, poster: 5.4,
    path: [
      [-450, 160, 150, 0, 135, 0],
      [-425, 166, 175, 0, 135, 0],
      [-400, 172, 200, 0, 135, 0],
    ],
  },
};

const STILLS: Record<string, Still> = {
  "zojoji.jpg": { view: [560, 95, 260, 0, 120, -10], field: 40, hour: 17.25 },
  "lattice.jpg": { view: [190, 120, 215, 0, 175, 0], field: 55, hour: 14 },
  "hamamatsucho.jpg": { view: [1320, 150, 660, 900, 10, 250], field: 50, hour: 11 },
  "night.jpg": { view: [-330, 210, 300, 0, 140, 0], field: 50, hour: 18.5 },
};

/** The share picture: the tower from the south-east as the lamps come on, at the size a link preview shows. */
const CARD = { file: "card.jpg", view: [330, 150, 330, 0, 150, 0] as Point, field: 50, hour: 17.86, width: 1200, height: 630 };

// ---------------------------------------------------------------- the pictures

async function run(command: string[], options: { cwd?: string; quiet?: boolean } = {}): Promise<string> {
  const child = Bun.spawn(command, { cwd: options.cwd ?? ROOT, stdin: "ignore", stdout: "pipe", stderr: options.quiet ? "pipe" : "inherit" });
  const [text, code] = await Promise.all([new Response(child.stdout).text(), child.exited]);
  if (code !== 0) throw new Error(`${command[0]} exited ${code}${options.quiet ? `: ${await new Response(child.stderr as ReadableStream).text()}` : ""}`);
  return text;
}

/** The frames of `lines` (the film binary's words) at a size, handed to ffmpeg with `encode` after its input. */
async function film(name: string, lines: string[], size: [number, number], encode: string[]): Promise<void> {
  if (!existsSync(PACK)) throw new Error(`${PACK} is missing: bun tools/wgpu.ts cook`);
  const list = join(WORK, `${name}.txt`);
  writeFileSync(list, lines.join("\n") + "\n");
  const camera = Bun.spawn([FILM, "--pack", PACK, "--frames", list, "--shape", "vita", "--size", size.join("x"), "--ticks", String(60 / RATE)], { stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const coder = Bun.spawn(["ffmpeg", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", size.join("x"), "-r", String(RATE), "-i", "-", ...encode], { stdin: camera.stdout, stdout: "inherit", stderr: "inherit" });
  const [said, filmed, coded] = await Promise.all([new Response(camera.stderr).text(), camera.exited, coder.exited]);
  if (filmed !== 0) throw new Error(`tokyo-film exited ${filmed} for ${name}: ${said.trim()}`);
  if (coded !== 0) throw new Error(`ffmpeg exited ${coded} for ${name}`);
  const trouble = /"trouble":"([^"]+)"/.exec(said)?.[1];
  if (trouble) throw new Error(`${name}: the renderer reported "${trouble}"`);
}

/** A JPEG of one frame. `-q:v 2` is ffmpeg's second finest quantiser; the chroma is not halved. */
const JPEG = ["-frames:v", "1", "-vf", "scale=in_range=pc:out_range=pc:out_color_matrix=bt601,format=yuvj444p", "-q:v", "2", "-map_metadata", "-1", "-fflags", "+bitexact"];

/** A point of a curve through `points` (Catmull-Rom), `along` from 0 to 1. */
function along(points: Point[], t: number): Point {
  const span = Math.min(points.length - 2, Math.floor(t * (points.length - 1)));
  const u = t * (points.length - 1) - span;
  const at = (i: number) => points[Math.max(0, Math.min(points.length - 1, i))];
  const [a, b, c, d] = [at(span - 1), at(span), at(span + 1), at(span + 2)];
  return b.map((_, k) => 0.5 * (2 * b[k] + (c[k] - a[k]) * u + (2 * a[k] - 5 * b[k] + 4 * c[k] - d[k]) * u * u + (3 * b[k] - a[k] - 3 * c[k] + d[k]) * u * u * u)) as Point;
}

const view = (point: Point, field = 55) => `view=${[...point.map((n) => n.toFixed(3)), field].join(",")}`;

/** A clip's frames as lines. The clock and the tour run by themselves from the first line. */
function lines(clip: Clip): string[] {
  const count = Math.round(clip.seconds * RATE);
  const rate = (clip.hours / clip.seconds).toFixed(5);
  const first = `hour=${clip.hour.toFixed(5)} rate=${rate} traffic=1 ` + (clip.path ? "tour=0" : `tour=1 at=${clip.tour} view=off`);
  return Array.from({ length: count }, (_, i) => [i === 0 ? first : "", clip.path ? view(along(clip.path, i / (count - 1))) : ""].join(" ").trim());
}

async function clip(file: string, take: Clip): Promise<void> {
  const frames = lines(take);
  // The rate is held under what keeps the file inside the Studio's limit, with a twentieth to spare.
  const most = Math.floor((LIMIT.video * 8 * 0.95) / take.seconds / 1000);
  await film(file, frames, [WIDTH, HEIGHT], [
    "-vf", `fade=t=in:s=0:n=${FADE},fade=t=out:s=${frames.length - FADE}:n=${FADE},scale=in_range=pc:out_range=tv:out_color_matrix=bt709,format=yuv420p`,
    "-c:v", "libx264", "-profile:v", "high", "-preset", "slow", "-crf", "20", "-maxrate", `${most}k`, "-bufsize", `${most * 2}k`, "-g", String(RATE * 2),
    "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv",
    "-an", "-movflags", "+faststart", "-map_metadata", "-1", "-fflags", "+bitexact", join(OUT, file),
  ]);
  // The poster is the clip filmed again up to that frame, and that frame alone kept: not a frame taken
  // back out of the encoded clip.
  const at = Math.min(frames.length - 1, Math.round(take.poster * RATE));
  await film(file.replace(/\.mp4$/, ".poster"), frames.slice(0, at + 1), [WIDTH, HEIGHT], ["-vf", `select=eq(n\\,${at}),scale=in_range=pc:out_range=pc:out_color_matrix=bt601,format=yuvj444p`, ...JPEG.filter((word, i, all) => word !== "-vf" && all[i - 1] !== "-vf"), join(OUT, file.replace(/\.mp4$/, ".jpg"))]);
}

async function still(file: string, take: Still, size: [number, number] = [WIDTH, HEIGHT]): Promise<void> {
  await film(file, [`tour=0 hour=${take.hour} rate=0 traffic=1 ${view(take.view, take.field)}`], size, [...JPEG, join(OUT, file)]);
}

// ---------------------------------------------------------------- the listing

interface Media {
  kind: "video" | "image";
  file: string;
  poster?: string;
  width: number;
  height: number;
  seconds?: number;
  from: string;
  caption: string;
}
interface Listing {
  tagline: string;
  description: string[];
  media: Media[];
  card: string;
}

async function probe(file: string): Promise<{ codec: string; pixels: string; width: number; height: number; seconds: number; streams: number; profile: string }> {
  const said = JSON.parse(await run(["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", file], { quiet: true }));
  const video = said.streams.find((stream: { codec_type: string }) => stream.codec_type === "video");
  return { codec: video.codec_name, pixels: video.pix_fmt, width: video.width, height: video.height, seconds: Number(said.format.duration ?? 0), streams: said.streams.length, profile: video.profile ?? "" };
}

/** Holds the directory to the contract: the words' lengths, every file named and there, its kind, its size. */
async function check(listing: Listing): Promise<{ file: string; bytes: number; seconds?: number }[]> {
  const faults: string[] = [], files: { file: string; bytes: number; seconds?: number }[] = [];
  const name = /^[a-z0-9-]+\.(mp4|jpg|webp|png)$/;
  if (!listing.tagline || listing.tagline.length > 120) faults.push("the tagline is one sentence of at most 120 characters");
  if (listing.description.length < 1 || listing.description.length > 6 || listing.description.some((p) => !p || p.length > 600)) faults.push("the description is one to six paragraphs of at most 600 characters");
  if (listing.media.length < 2 || listing.media.length > 12) faults.push("media has 2 to 12 entries");
  if (listing.media[0]?.kind !== "video") faults.push("the first entry is the lead clip");
  const picture = async (file: string, width: number, height: number, most: number) => {
    if (!name.test(file)) return void faults.push(`${file}: a file is named [a-z0-9-]+ with .mp4, .jpg, .webp or .png`);
    const path = join(OUT, file);
    if (!existsSync(path)) return void faults.push(`${file}: not filmed`);
    const bytes = statSync(path).size, seen = await probe(path);
    if (bytes > most) faults.push(`${file}: ${bytes} bytes, over ${most}`);
    if (seen.width !== width || seen.height !== height) faults.push(`${file}: ${seen.width} x ${seen.height}, and the listing says ${width} x ${height}`);
    files.push({ file, bytes });
  };
  for (const entry of listing.media) {
    if (!entry.caption || entry.caption.length > 140) faults.push(`${entry.file}: a caption has at most 140 characters`);
    if (!["browser", "psp", "vita", "3ds", "ipod-touch", "android"].includes(entry.from)) faults.push(`${entry.file}: from "${entry.from}"`);
    if (entry.width / entry.height < 4 / 3 || entry.width / entry.height > 2) faults.push(`${entry.file}: a picture is between 4:3 and 2:1`);
    if (entry.kind === "image") {
      await picture(entry.file, entry.width, entry.height, LIMIT.image);
      continue;
    }
    if (!name.test(entry.file) || !existsSync(join(OUT, entry.file))) {
      faults.push(`${entry.file}: not filmed`);
      continue;
    }
    const bytes = statSync(join(OUT, entry.file)).size, seen = await probe(join(OUT, entry.file));
    if (seen.codec !== "h264" || !["High", "Main"].includes(seen.profile) || seen.pixels !== "yuv420p" || seen.streams !== 1) faults.push(`${entry.file}: ${seen.codec} ${seen.profile} ${seen.pixels} in ${seen.streams} stream(s); a clip is H.264 High or Main, yuv420p, with no sound`);
    if (seen.width !== entry.width || seen.height !== entry.height) faults.push(`${entry.file}: ${seen.width} x ${seen.height}, and the listing says ${entry.width} x ${entry.height}`);
    if (Math.abs(seen.seconds - (entry.seconds ?? 0)) > 0.05) faults.push(`${entry.file}: ${seen.seconds} s, and the listing says ${entry.seconds}`);
    if (seen.seconds < 6 || seen.seconds > 30) faults.push(`${entry.file}: a clip is 6 to 30 seconds`);
    if (bytes > LIMIT.video) faults.push(`${entry.file}: ${bytes} bytes, over ${LIMIT.video}`);
    files.push({ file: entry.file, bytes, seconds: seen.seconds });
    if (!entry.poster) faults.push(`${entry.file}: a clip has a poster`);
    else await picture(entry.poster, entry.width, entry.height, LIMIT.image);
  }
  await picture(listing.card, CARD.width, CARD.height, LIMIT.card);
  const total = files.reduce((sum, file) => sum + file.bytes, 0);
  if (total > LIMIT.total) faults.push(`the directory is ${total} bytes, over ${LIMIT.total}`);
  if (faults.length) throw new Error(`listing: ${faults.join("\n         ")}`);
  return files;
}

async function upload(): Promise<void> {
  const link = join(ROOT, ".pocket-studio.json");
  if (!existsSync(link)) throw new Error(`no ${link}: run \`pocket-studio register --title "Pocket Tokyo"\` here, or copy the file of the checkout that did`);
  const project = JSON.parse(readFileSync(link, "utf8"));
  const cli = process.env.POCKET_STUDIO_CLI?.trim().split(/\s+/) ?? (Bun.which("pocket-studio") ? ["pocket-studio"] : null);
  if (!cli) throw new Error("no pocket-studio on PATH: install it from the Studio, or set POCKET_STUDIO_CLI to its command");
  console.log(`listing: uploading ${OUT} to ${project.server} (${project.app})`);
  const code = await Bun.spawn([...cli, "listing", OUT], { cwd: ROOT, stdin: "ignore", stdout: "inherit", stderr: "inherit" }).exited;
  if (code !== 0) throw new Error(`pocket-studio listing exited ${code}`);
}

// ---------------------------------------------------------------- main

const argv = process.argv.slice(2);
const only = argv.includes("--only") ? argv[argv.indexOf("--only") + 1] : null;
if (argv.includes("--help")) {
  console.log("usage: bun tools/listing.ts [--only FILE] [--upload]");
  process.exit(0);
}
const listing = JSON.parse(readFileSync(join(ROOT, "listing/listing.json"), "utf8")) as Listing;
const named = new Set([...listing.media.map((entry) => entry.file), listing.card]);
const takes = [...Object.keys(CLIPS), ...Object.keys(STILLS), CARD.file];
for (const file of named) if (!takes.includes(file)) throw new Error(`listing/listing.json names ${file}, and tools/listing.ts has no take for it`);
for (const file of takes) if (!named.has(file)) throw new Error(`tools/listing.ts films ${file}, and listing/listing.json does not name it`);
if (only && !takes.includes(only)) throw new Error(`no take named ${only}: ${takes.join(", ")}`);

mkdirSync(WORK, { recursive: true });
if (!only) rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT, { recursive: true });
await run(["cargo", "build", "--release", "--bin", "tokyo-film"], { cwd: CRATE });
const wanted = (file: string) => !only || only === file;
for (const [file, take] of Object.entries(CLIPS)) {
  if (!wanted(file)) continue;
  await clip(file, take);
  console.log(`listing: ${file}  ${take.seconds.toFixed(1)} s  ${(statSync(join(OUT, file)).size / MIB).toFixed(2)} MiB`);
}
for (const [file, take] of Object.entries(STILLS)) {
  if (!wanted(file)) continue;
  await still(file, take);
  console.log(`listing: ${file}`);
}
if (wanted(CARD.file)) {
  await still(CARD.file, CARD, [CARD.width, CARD.height]);
  console.log(`listing: ${CARD.file}`);
}
if (!only) {
  const files = await check(listing);
  writeFileSync(join(OUT, "listing.json"), JSON.stringify(listing, null, 2) + "\n");
  for (const file of files) console.log(`  ${file.file.padEnd(18)} ${String(file.bytes).padStart(9)} bytes${file.seconds ? `  ${file.seconds.toFixed(1)} s` : ""}`);
  console.log(`listing: ${files.length} files, ${(files.reduce((sum, file) => sum + file.bytes, 0) / MIB).toFixed(1)} MiB in ${OUT}`);
  if (argv.includes("--upload")) await upload();
} else if (argv.includes("--upload")) throw new Error("--upload sends the whole listing: run it without --only");
