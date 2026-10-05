// What crosses between a device's renderer and the interface. The renderer
// owns the flight and the scene; the interface owns every 2D pixel and what
// a button or a finger means while a list is up. Both sides speak JSON lines
// over PocketJS's in-process overlay service: the renderer sends the members
// of its state that changed, the interface sends commands.
// `crates/tokyo-interface` is the renderer's side.
//
// The renderers parse commands with a small scanner, so commands are flat
// objects of numbers, booleans and short strings.

export type Mode = "loading" | "title" | "flight" | "menu" | "error";

/** A switch (0 or 1), or with `choices` one of several named values. */
export interface Setting {
  key: string;
  value: number;
  choices?: string[];
}

export interface HostState {
  /** What the screen is for. Behind the title the tour flies. */
  mode: Mode;
  /** The loading step, or why `mode` is "error". */
  message: string;
  /** The tour carries the eye. */
  tour: boolean;
  /** What the device lets the person set, in menu order. */
  options: Setting[];
  /** One line of renderer statistics while the `stats` setting is on. */
  stats: string;
  /** What the interface last stored with `prefs`. */
  prefs: string;
  /** The `wake` the interface asked for that last came due; 0 before any has. */
  woke: number;
  /** The numbers that change in flight, at most once a turn (see `T`). */
  t: number[];
}

/** Indices into `HostState.t`. */
export const T = {
  /** Tokyo's clock, minutes since midnight. */
  minutes: 0,
  /** The eye's height, metres. */
  altitude: 1,
  /** km/h */
  speed: 2,
  /** Degrees clockwise from north. */
  heading: 3,
  /** The eye on the map, metres east and south of the area's origin. */
  x: 4,
  z: 5,
} as const;

/** The camera's buttons (`tokyo_sim::camera::btn`), for controls drawn on a touch panel. */
export const FLY = { fast: 1, up: 2, down: 4 } as const;

export type Command =
  /** Leave the title: on the tour, or with the eye in hand. */
  | { type: "start"; tour: boolean }
  /** A list comes up over the flight, or leaves it. */
  | { type: "menu"; on: boolean }
  /** Hand the eye to the tour, or take it. */
  | { type: "tour"; on: boolean }
  /** Turn Tokyo's clock to this many minutes after midnight. */
  | { type: "hour"; minutes: number }
  /** Back to the title. */
  | { type: "title" }
  | { type: "option"; key: string; value: number }
  /** Controls drawn on a touch panel: the stick, -100…100 on each axis (y is
   *  forward), and the held buttons (`FLY`). */
  | { type: "drive"; mx: number; my: number; b: number }
  /** A finger turning the view: logical px since the last command. */
  | { type: "look"; dx: number; dy: number }
  | { type: "prefs"; value: string }
  /** The interface has nothing scheduled (nothing fading). A turn of the
   *  guest costs milliseconds on the slower machines, so while this is on
   *  the renderer turns it only when it has news or a button the interface
   *  listens to changes. */
  | { type: "idle"; on: boolean }
  /** Say `id` back in `HostState.woke` in this many seconds. The interface's
   *  own timers count its turns, and an idle guest takes none: what must
   *  happen later (a hint leaving) is timed by the renderer. */
  | { type: "wake"; id: number; seconds: number };
