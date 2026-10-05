// The flight as the interface sees it, on any device: which list is up, what
// its rows do, where the eye is among the area's places, and what is kept
// between runs (the settings).
import { createEffect, createMemo, createSignal, on, onCleanup, untrack, type Accessor } from "solid-js";
import { after } from "@pocketjs/framework/clock";
import { glyph, modality, surfaceHasTouch } from "@pocketjs/framework/modality";
import { AREA, PLACES } from "./generated/area.ts";
import type { Host } from "./host.ts";
import { T, type Mode, type Setting } from "./protocol.ts";

/** What the settings a renderer may offer are called. */
const LABELS: Record<string, string> = {
  flow: "Clock",
  invert: "Invert look",
  traffic: "Traffic",
  stats: "Statistics",
};

/** Hours worth turning the clock to, by the sun of the day the city is lit for. */
export const HOURS: [string, number][] = [
  ["Dawn", 5 * 60 + 50],
  ["Morning", 9 * 60],
  ["Afternoon", 15 * 60],
  ["Sunset", 17 * 60 + 20],
  ["Dusk", 18 * 60 + 10],
  ["Night", 21 * 60],
];

/**
 * A row of a list. What can change while the row stands (a switch, a named
 * value, a label that says what a press would do) is a function: a list is
 * built once, kept, and its rows are written in place. Building a list takes
 * tenths of a second on the PSP.
 */
export interface Row {
  label: string | (() => string);
  /** A switch's state, or undefined for a row with a named value or none. */
  on?: () => boolean;
  /** The named value of a choice, or what stands at the right of the row. */
  value?: string | (() => string);
  /** A row that only informs takes no press and no focus mark. */
  press?: () => void;
}

/** What a row's member reads now. */
export function read<T extends string | boolean>(member: T | (() => T)): T {
  return typeof member === "function" ? member() : member;
}

/** The list over the city: the mode's own, or one opened from it. */
export type Sheet = "menu" | "time" | "settings" | "controls" | "about";

export interface Flight {
  host: Host;
  mode: Accessor<Mode>;
  /** True while a list is up and the pad is the interface's. */
  listing: Accessor<boolean>;
  sheet: Accessor<Sheet>;
  open(sheet: Sheet): void;
  /** The list's heading and rows. */
  heading: Accessor<string>;
  rows: Accessor<Row[]>;
  /** One level up: a list opened from another closes; the menu leaves the flight alone. */
  back(): void;
  /** Whether `back` does anything here, and what it is called. */
  backLabel: Accessor<string>;
  menu(): void;
  /** A line for the middle of the screen when the eye changes hands, "" when none. */
  note: Accessor<string>;
  /** The place the eye is at. */
  place: Accessor<string>;
  /** The title's own rows and the menu's: a presentation may build them before either is first shown. */
  lists: { title: Row[]; menu: Row[] };
  /** The renderer's `flow` setting, when it offers one. */
  flow: Accessor<Setting | undefined>;
  set(setting: Setting, value: number): void;
}

/** How many fades are playing: while one is, a timer of the guest's own is pending. */
const [settling, setSettling] = createSignal(0);
/** Seconds a fade takes to play, and a turn more. */
const SETTLE = 0.4;
/** Each showing of a pulse has an id of its own, which the renderer says back when the showing is over. */
let wakes = 0;
const waking = new Map<number, () => void>();

/** The guest takes every turn it is offered until what just changed has faded in or out. */
function settle() {
  setSettling((count) => count + 1);
  after(SETTLE, () => setSettling((count) => count - 1));
}

/**
 * True for `seconds` after each `show()`. The seconds are counted by the
 * renderer (`wake`), not by a timer here: a timer of the guest's counts its
 * turns, so one that is pending makes the device take every turn it offers,
 * 30 a second, for as long as a hint stands. Between the two fades nothing
 * is scheduled.
 */
export function createPulse(host: Host, seconds: number): [Accessor<boolean>, () => void] {
  const [shown, setShown] = createSignal(false);
  let id = 0;
  onCleanup(() => waking.delete(id));
  return [shown, () => {
    waking.delete(id);
    id = ++wakes;
    waking.set(id, () => {
      setShown(false);
      settle();
    });
    host.send({ type: "wake", id, seconds });
    if (untrack(shown)) return;
    setShown(true);
    settle();
  }];
}

/** "05:07" */
export function clock(minutes: number): string {
  const h = Math.floor(minutes / 60) % 24, m = Math.floor(minutes) % 60;
  return `${h < 10 ? "0" : ""}${h}:${m < 10 ? "0" : ""}${m}`;
}

/** What the sky is doing at an hour. */
export function phase(minutes: number): string {
  if (minutes < 5 * 60 + 20) return "NIGHT";
  if (minutes < 6 * 60 + 40) return "DAWN";
  if (minutes < 11 * 60) return "MORNING";
  if (minutes < 13 * 60) return "MIDDAY";
  if (minutes < 16 * 60 + 40) return "AFTERNOON";
  if (minutes < 17 * 60 + 50) return "SUNSET";
  if (minutes < 18 * 60 + 50) return "DUSK";
  return "NIGHT";
}

const POINTS = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
/** "NE" */
export function point(heading: number): string {
  return POINTS[Math.round(heading / 45) & 7];
}

/**
 * The place the eye is at: the named place nearest to where the view meets
 * the ground, when the eye is within its reach of that point; else the one
 * the eye is over; else the area.
 */
export function placeAt(t: number[]): string {
  const x = t[T.x], z = t[T.z], heading = (t[T.heading] * Math.PI) / 180;
  const ahead = Math.min(700, Math.max(0, t[T.altitude]) * 1.6);
  const ax = x + Math.sin(heading) * ahead, az = z - Math.cos(heading) * ahead;
  let best = "", least = Infinity;
  for (const place of PLACES) {
    const seen = Math.hypot(place.x - ax, place.z - az), over = Math.hypot(place.x - x, place.z - z);
    const d = Math.min(seen, over * 1.5);
    if (d < place.reach && d < least) {
      least = d;
      best = place.name;
    }
  }
  return best || AREA.name;
}

/** What flies the eye on this device, each control with what it does. */
export function controls(touch: boolean): [string, string][] {
  if (touch) {
    return [
      ["Stick", "Fly, slide sideways"],
      ["Drag", "Turn the view"],
      ["UP  DOWN", "Climb, descend"],
      ["FAST", "Hold to fly faster"],
      ["TOUR", "Hand the eye to the tour"],
      ["Clock", "Tap it to turn the hour"],
    ];
  }
  const dual = modality.screens.length > 1;
  // A Vita has a second stick; a PSP and a 3DS turn with the one they have and pitch with two buttons.
  const sticks: [string, string][] = !dual && surfaceHasTouch()
    ? [["Left stick", "Fly"], ["Right stick", "Turn the view"]]
    : [[dual ? "Circle Pad" : "Stick", "Fly forward, turn"], [`${glyph("triangle")} / ${glyph("cross")}`, "Look up, look down"]];
  return [
    ...sticks,
    [`${glyph("rtrigger")} / ${glyph("ltrigger")}`, "Climb, descend"],
    [glyph("square"), "Hold to fly faster"],
    ["D-pad", "Left, right: turn the clock"],
    [glyph("select"), "Tour on, tour off"],
    [glyph("start"), "Menu"],
  ];
}

interface Prefs {
  options?: Record<string, number>;
}

export function createFlight(host: Host, touch: boolean): Flight {
  const [sheet, setSheet] = createSignal<Sheet>("menu");
  const [prefs, setPrefs] = createSignal<Prefs>({});
  const [place, setPlace] = createSignal(AREA.name);
  const mode = host.mode;
  const listing = createMemo(() => mode() === "title" || mode() === "menu");

  // Each mode opens on its own list.
  createEffect(on(mode, () => setSheet("menu")));

  // The eye changing hands is said once, in the middle of the screen.
  const [noted, showNote] = createPulse(host, 2.2);
  createEffect(on(host.tour, () => mode() === "flight" && showNote(), { defer: true }));
  // With no pulse showing nothing is scheduled here: the renderer may skip turns until it has news.
  createEffect(() => host.ready() && host.send({ type: "idle", on: settling() === 0 }));
  // A showing of a pulse is over when the renderer says its id.
  createEffect(on(host.woke, (id) => {
    const over = waking.get(id);
    waking.delete(id);
    over?.();
  }, { defer: true }));

  // The place is worked out a few times a second from the numbers in flight; its name is a signal.
  let since = 0;
  onCleanup(host.onNumbers((t) => {
    if (since++ % 4) return;
    const now = placeAt(t);
    if (now !== untrack(place)) setPlace(now);
  }));

  // What the renderer stored for us last time: the settings, which it is told again once it has listed them.
  let restored = false;
  createEffect(() => {
    if (!host.ready() || restored || !host.options().length) return;
    restored = true;
    let stored: Prefs = {};
    try {
      stored = JSON.parse(untrack(host.prefs) || "{}") as Prefs;
    } catch {
      // A damaged file starts over.
    }
    setPrefs(stored);
    for (const setting of untrack(host.options)) {
      const value = stored.options?.[setting.key];
      if (typeof value === "number" && value !== setting.value) host.send({ type: "option", key: setting.key, value });
    }
  });
  const set = (setting: Setting, value: number) => {
    host.send({ type: "option", key: setting.key, value });
    const next = { ...prefs(), options: { ...prefs().options, [setting.key]: value } };
    setPrefs(next);
    host.send({ type: "prefs", value: JSON.stringify(next) });
  };
  // A setting's row stands while the renderer offers the setting: its state is read where it is shown.
  const row = (key: string, choice: boolean): Row => {
    const label = LABELS[key] ?? key;
    const now = () => host.options().find((setting) => setting.key === key);
    if (!choice) {
      return { label, on: () => !!now()?.value, press: () => {
        const setting = now();
        if (setting) set(setting, setting.value ? 0 : 1);
      } };
    }
    return { label, value: () => now()?.choices?.[now()!.value] ?? "", press: () => {
      const setting = now();
      if (setting?.choices) set(setting, (setting.value + 1) % setting.choices.length);
    } };
  };
  const flow = createMemo(() => host.options().find((setting) => setting.key === "flow"));
  // Which settings the renderer offers, each with whether it is a choice: the lists made of them are
  // built again only when this changes, not when a setting's value does.
  const offered = createMemo(() => host.options().map((setting) => `${setting.key}${setting.choices ? ":" : ""}`).join(" "));
  // The clock's own setting stands with the hours; the rest are the settings.
  const settings = createMemo<Row[]>(() => offered().split(" ").filter((key) => key && key !== "flow:").map((key) => row(key.replace(":", ""), key.endsWith(":"))));
  const hours = createMemo<Row[]>(() => [
    ...HOURS.map(([label, minutes]): Row => ({ label, value: clock(minutes), press: () => host.send({ type: "hour", minutes }) })),
    ...(offered().split(" ").includes("flow:") ? [row("flow", true)] : []),
  ]);

  // Each mode's own list is built once: showing it again changes no row.
  const more: Row[] = [
    { label: "Time of day", press: () => setSheet("time") },
    { label: "Settings", press: () => setSheet("settings") },
  ];
  const title: Row[] = [
    { label: "Take the tour", press: () => host.send({ type: "start", tour: true }) },
    { label: "Fly yourself", press: () => host.send({ type: "start", tour: false }) },
    ...more,
    { label: "About", press: () => setSheet("about") },
  ];
  const leave = () => host.send({ type: "menu", on: false });
  const menu: Row[] = [
    // On a touch panel the way back stands in the heading.
    ...(touch ? [] : [{ label: "Resume", press: leave }]),
    { label: () => (host.tour() ? "Fly yourself" : "Join the tour"), press: () => (host.send({ type: "tour", on: !host.tour() }), leave()) },
    ...more,
    { label: touch ? "How to fly" : "Controls", press: () => setSheet("controls") },
    { label: "Back to the title", press: () => host.send({ type: "title" }) },
  ];
  const explained: Row[] = controls(touch).map(([label, value]) => ({ label, value }));
  const about: Row[] = [
    { label: "Place", value: AREA.district ? `${AREA.name}, ${AREA.district}` : AREA.name },
    { label: "Buildings", value: "PLATEAU, MLIT Japan" },
    { label: "Streets", value: "© OpenStreetMap contributors" },
    { label: "Terrain", value: "GSI Japan" },
    { label: "Model", value: "Procedural Tokyo, Yong Su" },
    { label: "Engine", value: "Pocket3D" },
  ];
  // During a flight no list is up: the rows stay what they were, so nothing is rebuilt under the gauges.
  const rows = createMemo<Row[]>((before) => {
    if (!listing()) return before;
    if (sheet() === "time") return hours();
    if (sheet() === "settings") return settings();
    if (sheet() === "controls") return explained;
    if (sheet() === "about") return about;
    return mode() === "title" ? title : menu;
  }, []);

  return {
    host, mode, listing, sheet, rows, place, flow, set,
    lists: { title, menu },
    open: setSheet,
    // A note belongs to the flight: it does not stand under a list.
    note: () => (mode() === "flight" && noted() ? (host.tour() ? "Tour" : "Free flight") : ""),
    heading: () => {
      if (sheet() === "time") return "TIME OF DAY";
      if (sheet() === "settings") return "SETTINGS";
      if (sheet() === "controls") return touch ? "HOW TO FLY" : "CONTROLS";
      if (sheet() === "about") return "ABOUT";
      return mode() === "menu" ? "MENU" : "";
    },
    back() {
      if (sheet() !== "menu") setSheet("menu");
      else if (mode() === "menu") leave();
    },
    backLabel: () => (sheet() !== "menu" ? "back" : mode() === "menu" ? "resume" : ""),
    menu: () => host.send({ type: "menu", on: true }),
  };
}
