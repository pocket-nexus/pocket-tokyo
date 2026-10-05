// The pieces every presentation is built from. A device chooses where they
// go and how large they are; what the clock, the compass, a list row or the
// map looks like is decided here once.
//
// The numbers in flight (the clock, the height, the heading, the eye on the
// map) change up to 30 times a second. Each is written straight to its node
// (`@pocketjs/framework/hot`) in a cell of fixed size, so a new value costs
// one native call and no layout. Everything else is a signal.
import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, untrack, type JSX } from "solid-js";
import { useActions, type ActionsHandle } from "@pocketjs/framework/actions";
import { Image, Text, View } from "@pocketjs/framework/components";
import type { SurfaceId } from "@pocketjs/framework/display";
import { createGesture } from "@pocketjs/framework/gesture";
import { prop as hotProp, text as hotText } from "@pocketjs/framework/hot";
import { BTN } from "@pocketjs/framework/input";
import { onFrame } from "@pocketjs/framework/lifecycle";
import { modality } from "@pocketjs/framework/modality";
import type { NodeMirror } from "@pocketjs/framework/renderer";
import { clock, phase, point, read, type Flight, type Row } from "./flight.ts";
import { AREA, MAP, PLACES } from "./generated/area.ts";
import type { Host } from "./host.ts";
import { T } from "./protocol.ts";
import { DIM, FAINT, GLASS, HAIRLINE, INK, NIGHT, PANEL, PLATE, tint, TOWER, WASH } from "./theme.ts";

/**
 * A view a finger can tap, on the surface it is drawn on. It stays out of
 * the focus order: a pad has its own button for the same verb.
 */
export function Touchable(props: { surface?: SurfaceId; onTap?: () => void; class?: string; style?: Record<string, number | string>; children?: JSX.Element }) {
  let node: NodeMirror | undefined;
  const [down, setDown] = createSignal(false);
  createGesture({
    surface: props.surface,
    region: { node: () => node },
    onDown: () => setDown(!!props.onTap),
    onUp: () => setDown(false),
    onCancel: () => setDown(false),
    onTap: () => props.onTap?.(),
  });
  return <View ref={node} class={props.class} style={{ ...props.style, opacity: down() ? 0.55 : 1 }}>{props.children}</View>;
}

/** A button for a finger: at least as tall as a fingertip on its surface. */
export function Button(props: { label: string; width: number; height: number; strong?: boolean; surface?: SurfaceId; onPress: () => void }) {
  return (
    <Touchable surface={props.surface} class="rounded-lg items-center justify-center" style={{ width: props.width, height: props.height, bgColor: props.strong ? TOWER : "#0b0f15b0", borderWidth: 1, borderColor: props.strong ? TOWER : HAIRLINE }} onTap={props.onPress}>
      <Text class="text-sm font-bold" style={{ textColor: props.strong ? NIGHT : INK }}>{props.label}</Text>
    </Touchable>
  );
}

/** A pill of text over the scene. */
export function Chip(props: { text: string; color?: string }) {
  return (
    <View class="rounded px-2 py-1" style={{ bgColor: "#0b0f15b0" }}>
      <Text class="text-xs font-bold" style={{ textColor: props.color ?? INK }}>{props.text}</Text>
    </View>
  );
}

/** The name, as large as the surface has room for. */
export function Wordmark(props: { large?: boolean }) {
  return (
    <View class="flex-col">
      <Text class="text-sm font-bold tracking-wide" style={{ textColor: TOWER }}>POCKET</Text>
      <Show when={props.large} fallback={<Text class="text-2xl font-bold" style={{ textColor: INK }}>TOKYO</Text>}>
        <Text class="text-4xl font-bold" style={{ textColor: INK }}>TOKYO</Text>
      </Show>
    </View>
  );
}

/** Dark glass a little larger than a readout, behind it: white type stays
 *  legible over a bright sky or a pale roof. */
function Plate(props: { width: number; height: number }) {
  return <View class="absolute rounded" style={{ insetL: -6, insetT: -4, width: props.width + 12, height: props.height + 8, bgColor: PLATE }} />;
}

/**
 * Tokyo's clock: what the sky is doing over the hour and the minute; where
 * `compact`, the hour and the minute alone (a column 65 pixels wide holds no
 * "AFTERNOON"). The digits stand at the left of a cell of fixed size: a
 * number written to its node keeps the place layout gave the last one, so
 * text that is centred or set to the right would drift.
 */
export function Clock(props: { host: Host; compact?: boolean }) {
  let digits: NodeMirror | undefined, word: NodeMirror | undefined;
  let shown = -1, named = "";
  onMount(() => onCleanup(props.host.onNumbers((t) => {
    const minutes = t[T.minutes];
    if (minutes === shown) return;
    shown = minutes;
    hotText(digits, clock(minutes));
    const now = phase(minutes);
    if (now !== named) hotText(word, (named = now));
  })));
  const width = () => (props.compact ? 52 : 76);
  return (
    <View class="relative flex-col" style={{ width: width() }}>
      <Plate width={width()} height={props.compact ? 24 : 46} />
      <Show when={!props.compact}>
        <Text ref={word} class="text-xs font-bold tracking-wide" style={{ width: width(), height: 14, textColor: DIM }}>MIDDAY</Text>
      </Show>
      <Text ref={digits} class={props.compact ? "text-lg font-bold" : "text-2xl font-bold"} style={{ width: width(), height: props.compact ? 24 : 30, textColor: INK }}>12:00</Text>
    </View>
  );
}

/** Where the eye is: what the area lies in, and the place under the view. While
 *  the tour carries the eye a mark says so. */
export function Place(props: { flight: Flight; width: number }) {
  const host = props.flight.host;
  // The name is written to its node: a new place lays nothing out.
  let name: NodeMirror | undefined;
  createEffect(() => hotText(name, props.flight.place()));
  return (
    <View class="relative flex-col" style={{ width: props.width }}>
      <Plate width={props.width} height={40} />
      <View class="flex-row items-center gap-2" style={{ height: 14 }}>
        <Text class="text-xs font-bold tracking-wide" style={{ textColor: DIM }}>{(AREA.district || AREA.name).toUpperCase()}</Text>
        <Show when={host.tour()}>
          <View class="flex-row items-center gap-1">
            <View class="rounded-full w-[6] h-[6]" style={{ bgColor: TOWER }} />
            <Text class="text-xs font-bold tracking-wide" style={{ textColor: TOWER }}>TOUR</Text>
          </View>
        </Show>
      </View>
      <Text ref={name} class="text-lg font-bold" style={{ width: props.width, height: 24, textColor: INK }}>{AREA.name}</Text>
    </View>
  );
}

/** A number in flight under its unit: the eye's height in metres, or its speed. */
export function Readout(props: { host: Host; of: "altitude" | "speed"; compact?: boolean }) {
  let digits: NodeMirror | undefined;
  let shown = -1;
  onMount(() => onCleanup(props.host.onNumbers((t) => {
    const value = t[T[props.of]];
    if (value === shown) return;
    shown = value;
    hotText(digits, value);
  })));
  const width = () => (props.compact ? 52 : 68);
  return (
    <View class="relative flex-col" style={{ width: width() }}>
      <Plate width={width()} height={props.compact ? 38 : 46} />
      <Text class="text-xs font-bold tracking-wide" style={{ textColor: DIM }}>{props.of === "altitude" ? "ALT M" : "KM/H"}</Text>
      <Text ref={digits} class={props.compact ? "text-lg font-bold" : "text-2xl font-bold"} style={{ width: width(), height: props.compact ? 24 : 30, textColor: INK }}>0</Text>
    </View>
  );
}

/** Pixels of tape per degree of heading. */
const TAPE = 1;
const POINTS = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
/** Steps a mark of the tape fades through at an end of the window. */
const FADE = 4;

/**
 * The heading as a tape: the points of the compass slide under a mark at the
 * middle, and the bearing stands under it in degrees. The tape is one node
 * that moves; a new heading lays nothing out.
 *
 * The window's ends cut the tape with a clip, which a GE, a PICA200 and
 * OpenGL ES do with a scissor. With `fade` no clip is used: a mark that
 * reaches an end fades out, which is one more write to its node. That is for
 * the Vita, whose host draws a clip as a pass over the whole screen's
 * stencil, in the scene the city is drawn in; the comparisons it takes each
 * turn are a millisecond of a PSP's.
 */
export function Compass(props: { host: Host; width: number; fade?: boolean }) {
  let tape: NodeMirror | undefined, degrees: NodeMirror | undefined;
  let shown = -1;
  // The points from a quarter turn before north to a quarter past the next north: the window never
  // looks past either end. Between two points stands a tick.
  const marks = Array.from({ length: 13 }, (_, i) => ({ at: (i - 2) * 45, label: undefined as NodeMirror | undefined, tick: undefined as NodeMirror | undefined, steps: [-1, -1] }));
  /** How much of a mark `offset` degrees from the heading shows, in steps of the fade. */
  const step = (offset: number) => (props.fade ? Math.max(0, Math.min(FADE, Math.round((props.width / 2 - 6 - Math.abs(offset) * TAPE) / 4))) : FADE);
  const fade = (heading: number) => {
    for (const mark of marks) {
      const now = [step(mark.at - heading), step(mark.at + 22 - heading)];
      if (now[0] !== mark.steps[0]) hotProp(mark.label, "opacity", (mark.steps[0] = now[0]) / FADE);
      if (now[1] !== mark.steps[1]) hotProp(mark.tick, "opacity", (mark.steps[1] = now[1]) / FADE);
    }
  };
  onMount(() => onCleanup(props.host.onNumbers((t) => {
    const heading = t[T.heading];
    if (heading === shown) return;
    shown = heading;
    hotProp(tape, "translateX", Math.round(props.width / 2 - heading * TAPE));
    if (props.fade) fade(heading);
    hotText(degrees, `${heading < 10 ? "00" : heading < 100 ? "0" : ""}${heading}° ${point(heading)}`);
  })));
  return (
    <View class="relative" style={{ width: props.width, height: 38 }}>
      <Plate width={props.width} height={38} />
      <View class={props.fade ? "absolute" : "absolute overflow-hidden"} style={{ insetL: 0, insetT: 0, width: props.width, height: 20 }}>
        <View ref={tape} class="absolute" style={{ insetL: 0, insetT: 0, width: 1, height: 20, translateX: Math.round(props.width / 2) }}>
          <For each={marks}>
            {(mark) => (
              <>
                <Text ref={mark.label} class="absolute text-xs font-bold text-center" style={{ insetL: mark.at * TAPE - 12, insetT: 5, width: 24, opacity: step(mark.at) / FADE, textColor: mark.at % 90 === 0 ? INK : DIM }}>{POINTS[(((mark.at / 45) % 8) + 8) % 8]}</Text>
                <View ref={mark.tick} class="absolute" style={{ insetL: mark.at * TAPE + 22, insetT: 9, width: 1, height: 5, opacity: step(mark.at + 22) / FADE, bgColor: FAINT }} />
              </>
            )}
          </For>
        </View>
      </View>
      <View class="absolute" style={{ insetL: props.width / 2 - 1, insetT: 0, width: 2, height: 5, bgColor: TOWER }} />
      <Text ref={degrees} class="absolute text-xs" style={{ insetL: props.width / 2 - 24, insetT: 22, width: 60, height: 14, textColor: DIM }}>000° N</Text>
    </View>
  );
}

/** The line in the middle of the screen when the eye changes hands. */
export function Note(props: { flight: Flight; width: number }) {
  return (
    <View class={props.flight.note() ? "items-center justify-center opacity-100 transition-opacity duration-200" : "items-center justify-center opacity-0 transition-opacity duration-200"} style={{ width: props.width }}>
      <View class="rounded px-3 py-1" style={{ bgColor: "#0b0f1580" }}>
        <Text class="text-lg font-bold" style={{ textColor: INK }}>{props.flight.note() || " "}</Text>
      </View>
    </View>
  );
}

/** The strip along the bottom of a screen with buttons: a notice at the left,
 *  what the buttons do at the right. */
export function Legend(props: { width: number; left?: string; legend: string }) {
  return (
    <View class="relative" style={{ width: props.width, height: 24, bgColor: GLASS }}>
      <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
      <Text class="absolute text-xs" style={{ insetL: 10, insetT: 5, textColor: DIM }}>{props.left ?? ""}</Text>
      <Text class="absolute text-xs" style={{ insetR: 10, insetT: 5, textColor: INK }}>{props.legend}</Text>
    </View>
  );
}

/** The renderer's statistics line, while its setting is on. */
export function Stats(props: { host: Host }) {
  return (
    <Show when={props.host.stats()}>
      <Chip text={props.host.stats()} color={DIM} />
    </Show>
  );
}

/** A list's focus, and the buttons that drive it. One for the life of a
 *  presentation: it watches the pad through the flight too, so a button held
 *  when a list opens is not taken for a press. */
export interface Menu {
  focus: () => number;
  press(index: number): void;
  actions: ActionsHandle;
}

export function createMenu(flight: Flight): Menu {
  const [focus, setFocus] = createSignal(0);
  const pressable = (row: Row | undefined) => !!row?.press;
  // A list starts on its first row; a switch that changes under the focus keeps it.
  createEffect(on(() => `${flight.mode()} ${flight.sheet()}`, () => setFocus(0)));
  const press = (index: number) => {
    const row = flight.rows()[index];
    if (!pressable(row)) return;
    setFocus(index);
    row!.press!();
  };
  const step = (by: number) => {
    const rows = flight.rows();
    for (let at = focus() + by; at >= 0 && at < rows.length; at += by) {
      if (pressable(rows[at])) return setFocus(at);
    }
  };
  let previous = ~0;
  onFrame((buttons) => {
    const pressed = buttons & ~previous;
    previous = buttons;
    if (!flight.listing()) return;
    if (pressed & BTN.UP) step(-1);
    if (pressed & BTN.DOWN) step(1);
  });
  // The bindings are read every turn: built again only when the mode or the list changes.
  const actions = useActions(createMemo(() => ({
    confirm: { label: "select", run: () => press(focus()), when: () => flight.listing() && pressable(flight.rows()[focus()]) },
    back: { label: flight.backLabel(), run: flight.back, when: () => flight.listing() && !!flight.backLabel() },
    // START opens the menu over a flight, and closes it.
    media: { label: flight.mode() === "flight" ? "menu" : undefined, run: () => (flight.mode() === "menu" ? flight.back() : flight.menu()), when: () => flight.mode() === "flight" || (flight.mode() === "menu" && flight.sheet() === "menu") },
  })));
  return { focus, press, actions };
}

/** A switch: its knob at the right when on. */
function Switch(props: { on: boolean }) {
  return (
    <View class="relative rounded-full w-[34] h-[18]" style={{ bgColor: props.on ? TOWER : "#ffffff30" }}>
      <View class="absolute rounded-full w-[14] h-[14] transition-transform duration-150" style={{ insetL: 2, insetT: 2, translateX: props.on ? 16 : 0, bgColor: props.on ? NIGHT : INK }} />
    </View>
  );
}

/**
 * The rows of the list that is up. A row that acts takes a tap and, where
 * the device has buttons, the focus mark: one bar that slides to the row,
 * so moving the focus changes one node. A row that only informs is a label
 * and what it means, in a shorter line.
 *
 * Every list this view has shown stays built, hidden: showing one again
 * builds nothing, and a switch that turns writes its own row. `warm` lists
 * are built with the view, before they are first shown.
 */
export function Rows(props: { flight: Flight; menu: Menu; width: number; rowHeight: number; infoHeight?: number; surface?: SurfaceId; active?: () => boolean; warm?: Row[][] }) {
  const info = () => props.infoHeight ?? 22;
  // A list that is kept but hidden (`active` false) holds its rows and its mark as they were.
  const live = () => props.active?.() ?? true;
  const rows = createMemo<Row[]>((before) => (live() ? props.flight.rows() : before), props.warm?.[0] ?? []);
  const focus = createMemo<number>((before) => (live() ? props.menu.focus() : before), 0);
  const built = createMemo<Row[][]>((before) => (!rows().length || before.includes(rows()) ? before : [...before, rows()]), props.warm ?? []);
  return (
    <View class="relative flex-col" style={{ width: props.width }}>
      <Show when={modality.buttons && rows().some((row) => row.press)}>
        <View class="absolute transition-transform duration-100 ease-out" style={{ insetL: 0, insetT: 0, width: props.width, height: props.rowHeight, translateY: focus() * props.rowHeight, bgColor: WASH }}>
          <View class="absolute" style={{ insetL: 0, insetT: 0, width: 3, height: props.rowHeight, bgColor: TOWER }} />
        </View>
      </Show>
      <For each={built()}>
        {(list) => {
          // A list keeps the row height it was built with: another list's height does not reach it.
          const height = untrack(() => props.rowHeight);
          return (
            <View class="flex-col" style={{ width: props.width, display: list === rows() ? 0 : 1 }}>
              <For each={list}>
                {(row, index) => (
                  <Show
                    when={row.press}
                    fallback={
                      <View class="relative" style={{ width: props.width, height: info() }}>
                        <Text class="absolute text-xs font-bold" style={{ insetL: 14, insetT: info() / 2 - 8, textColor: TOWER }}>{read(row.label)}</Text>
                        <Text class="absolute text-xs" style={{ insetL: 14 + Math.min(104, props.width * 0.36), insetT: info() / 2 - 8, textColor: DIM }}>{read(row.value ?? "")}</Text>
                      </View>
                    }
                  >
                    <Touchable surface={props.surface} class="relative" style={{ width: props.width, height }} onTap={() => props.menu.press(index())}>
                      <Text class="absolute text-sm font-bold" style={{ insetL: 14, insetT: height / 2 - 9, textColor: INK }}>{read(row.label)}</Text>
                      <Show when={row.on}>
                        <View class="absolute" style={{ insetR: 14, insetT: height / 2 - 9 }}><Switch on={row.on!()} /></View>
                      </Show>
                      <Show when={row.value !== undefined}>
                        <Text class="absolute text-sm" style={{ insetR: 14, insetT: height / 2 - 9, textColor: DIM }}>{read(row.value!)}</Text>
                      </Show>
                      <View class="absolute" style={{ insetL: 0, insetB: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
                    </Touchable>
                  </Show>
                )}
              </For>
            </View>
          );
        }}
      </For>
    </View>
  );
}

/**
 * A screen that is built the first time `when` holds (at once with `eager`)
 * and then stays, shown or hidden. Showing it again builds nothing: on the
 * PSP a screen takes tenths of a second to build, which the loading screen
 * can spend and the moment a list closes cannot.
 */
export function Keep(props: { when: boolean; eager?: boolean; width: number; height: number; children: JSX.Element }) {
  const [built, setBuilt] = createSignal(!!props.eager);
  createEffect(() => {
    if (props.when) setBuilt(true);
  });
  return (
    <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: props.height, display: props.when ? 0 : 1, hitPass: 1 }}>
      <Show when={built()}>{props.children}</Show>
    </View>
  );
}

/** A list's heading over a rule. On a surface a finger reaches, `back`
 *  stands at its left as the way out of the list. */
export function Heading(props: { text: string; width: number; height: number; back?: string; onBack?: () => void; surface?: SurfaceId }) {
  const way = () => (props.back ? 88 : 0);
  return (
    <View class="relative" style={{ width: props.width, height: props.height }}>
      <Show when={props.back}>
        <Touchable surface={props.surface} class="absolute flex-row items-center" style={{ insetL: 0, insetT: 0, width: 96, height: props.height }} onTap={props.onBack}>
          <Text class="text-sm font-bold" style={{ marginL: 14, textColor: TOWER }}>{`‹ ${props.back}`}</Text>
        </Touchable>
      </Show>
      <Text class="absolute text-xs font-bold tracking-wide" style={{ insetL: 14 + way(), insetT: props.height / 2 - 8, textColor: DIM }}>{props.text}</Text>
      <View class="absolute" style={{ insetL: 0, insetB: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
    </View>
  );
}

/** A view that fades with `shown`. */
export function Fade(props: { shown: boolean; children: JSX.Element }) {
  return <View class={props.shown ? "opacity-100 transition-opacity duration-300" : "opacity-0 transition-opacity duration-300"}>{props.children}</View>;
}

/** A panel over a scrim, in the middle of a surface. */
export function Panel(props: { width: number; height: number; panelWidth: number; panelHeight: number; children: JSX.Element }) {
  return (
    <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: props.height, bgColor: "#00000080" }}>
      <View class="absolute rounded-lg overflow-hidden flex-row" style={{ insetL: (props.width - props.panelWidth) / 2, insetT: Math.max(4, (props.height - props.panelHeight) / 2), width: props.panelWidth, height: props.panelHeight, bgColor: PANEL, borderWidth: 1, borderColor: HAIRLINE }}>
        {props.children}
      </View>
    </View>
  );
}

/**
 * The area from above with its places and the eye on it. North is up. The
 * eye's mark moves and turns with the numbers in flight; the tower's place
 * carries its colour.
 */
export function AreaMap(props: { host: Host; width: number }) {
  const scale = () => props.width / MAP.width;
  const height = () => Math.round(MAP.height * scale());
  const px = (metres: number, from: number) => ((metres - from) / MAP.metres) * scale();
  let eye: NodeMirror | undefined;
  onMount(() => onCleanup(props.host.onNumbers((t) => {
    hotProp(eye, "translateX", Math.round(Math.max(0, Math.min(props.width, px(t[T.x], MAP.x0)))));
    hotProp(eye, "translateY", Math.round(Math.max(0, Math.min(height(), px(t[T.z], MAP.z0)))));
    hotProp(eye, "rotate", t[T.heading]);
  })));
  return (
    <View class="relative overflow-hidden" style={{ width: props.width, height: height(), bgColor: "#2c3036" }}>
      <Image src={MAP.image} class="absolute" style={{ insetL: 0, insetT: 0, width: MAP.texture * scale(), height: MAP.texture * scale() }} />
      <For each={PLACES}>
        {(place, index) => (
          <View class="absolute" style={{ insetL: px(place.x, MAP.x0) - (index() ? 1.5 : 3), insetT: px(place.z, MAP.z0) - (index() ? 1.5 : 3), width: index() ? 3 : 6, height: index() ? 3 : 6, radius: index() ? 1.5 : 3, bgColor: index() ? "#f6f3eab0" : TOWER, borderWidth: index() ? 0 : 1, borderColor: NIGHT }} />
        )}
      </For>
      {/* The eye: a square with a line toward where the view faces. */}
      <View ref={eye} class="absolute" style={{ insetL: -5, insetT: -5, width: 10, height: 10 }}>
        <View class="absolute" style={{ insetL: 4, insetT: -7, width: 2, height: 9, bgColor: INK }} />
        <View class="absolute" style={{ insetL: 2, insetT: 2, width: 6, height: 6, bgColor: "#ffe14d", borderWidth: 1, borderColor: NIGHT }} />
      </View>
    </View>
  );
}

/** The day as a bar: night, the two twilights and daylight, by the sun the city is lit for. */
const DAY: [number, string][] = [[0, "#1b2440"], [5.4, "#c2764a"], [6.5, "#7eb0dc"], [17, "#d8763c"], [18.2, "#3a3566"], [19, "#1b2440"], [24, ""]];

/**
 * The clock as a bar a finger turns: the day from midnight to midnight, with
 * a knob at the hour. `left` is the bar's left edge on its surface. The knob
 * follows the finger while it is down and the clock otherwise.
 */
export function TimeBar(props: { flight: Flight; width: number; left: number; surface?: SurfaceId }) {
  const host = props.flight.host;
  const INSET = 14;
  const span = () => props.width - INSET * 2;
  let area: NodeMirror | undefined, knob: NodeMirror | undefined;
  let held = false, sent = -1;
  const turn = (c: { x: number }) => {
    const part = Math.max(0, Math.min(1, (c.x - props.left - INSET) / span()));
    hotProp(knob, "translateX", Math.round(part * span()));
    const minutes = Math.min(1435, Math.round((part * 1440) / 5) * 5);
    if (minutes === sent) return;
    sent = minutes;
    host.send({ type: "hour", minutes });
  };
  const release = () => {
    held = false;
    sent = -1;
  };
  createGesture({
    surface: props.surface,
    region: { node: () => area },
    tapSlop: 9999,
    onDown: (c) => {
      held = true;
      turn(c);
    },
    onMove: turn,
    onUp: release,
    onCancel: release,
  });
  onMount(() => onCleanup(host.onNumbers((t) => {
    if (!held) hotProp(knob, "translateX", Math.round((t[T.minutes] / 1440) * span()));
  })));
  return (
    <View ref={area} class="relative" style={{ width: props.width, height: 36 }}>
      <View class="absolute rounded overflow-hidden flex-row" style={{ insetL: INSET, insetT: 9, width: span(), height: 8 }}>
        <For each={DAY.slice(0, -1)}>
          {([from, color], index) => <View style={{ width: ((DAY[index() + 1][0] - from) / 24) * span(), height: 8, bgColor: color }} />}
        </For>
      </View>
      <For each={[0, 6, 12, 18]}>
        {(hour) => <Text class="absolute text-xs" style={{ insetL: INSET + (hour / 24) * span() - 1, insetT: 20, textColor: FAINT }}>{hour < 10 ? `0${hour}` : `${hour}`}</Text>}
      </For>
      <View ref={knob} class="absolute rounded-full w-[16] h-[16]" style={{ insetL: INSET - 8, insetT: 5, bgColor: INK, borderWidth: 3, borderColor: TOWER }} />
    </View>
  );
}

/** The screen while the city loads, and when it could not. */
export function Loading(props: { host: Host; width: number; height: number }) {
  return (
    <View class="items-center justify-center flex-col gap-3" style={{ width: props.width, height: props.height, bgColor: NIGHT }}>
      <Wordmark />
      <Text class="text-xs" style={{ textColor: props.host.mode() === "error" ? TOWER : DIM }}>{props.host.message() || "Reading the city"}</Text>
    </View>
  );
}

export { tint };
