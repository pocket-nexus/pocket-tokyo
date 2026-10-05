// A touch panel and nothing else (the iPod touch, held sideways): every verb
// is a control under a thumb. During a flight a stick stands in the lower
// left to fly, the keys in the lower right climb, descend and speed up, and
// a finger anywhere else turns the view. The clock opens the day as a bar to
// drag. Lists are buttons a finger presses.
import { createEffect, createSignal, For, on, Show, type JSX } from "solid-js";
import { Text, View } from "@pocketjs/framework/components";
import { createGesture } from "@pocketjs/framework/gesture";
import { onFrame } from "@pocketjs/framework/lifecycle";
import type { NodeMirror } from "@pocketjs/framework/renderer";
import { clock, createFlight, createPulse, HOURS, type Flight } from "../flight.ts";
import { connectHost, type Host } from "../host.ts";
import { AreaMap, Button, Chip, Clock, Compass, createMenu, Fade, Heading, Keep, Loading, type Menu, Note, Panel, Place, Readout, Rows, Stats, TimeBar, Touchable, Wordmark } from "../parts.tsx";
import { FLY } from "../protocol.ts";
import { DIM, GLASS, HAIRLINE, INK, NIGHT, tint, TOWER } from "../theme.ts";

let W = 480, H = 320;
/** The panel's own size, said before the presentation mounts when it is not the iPod touch's (`main-touch-wide.tsx`). */
export function panel(width: number, height: number) {
  W = width;
  H = height;
}
/** A fingertip on this panel (Pocket HIG, touch modality). */
const TARGET = 44;

export default function TouchScreen() {
  const host = connectHost();
  const flight = createFlight(host, true);
  const menu = createMenu(flight);
  return (
    <View class="relative w-full h-full">
      <Show when={host.mode() === "loading" || host.mode() === "error"}><Loading host={host} width={W} height={H} /></Show>
      <Keep when={host.mode() === "title"} eager width={W} height={H}><Title flight={flight} menu={menu} /></Keep>
      {/* The controls stay built under the menu: opening it and closing it build nothing. */}
      <Keep when={host.mode() === "flight"} eager width={W} height={H}><Controls flight={flight} /></Keep>
      <Keep when={host.mode() === "menu"} eager width={W} height={H}><Over flight={flight} menu={menu} /></Keep>
    </View>
  );
}

/** The hours as keys and the day as a bar, for a list a finger works. `left`: the bar's left edge on the panel. */
function Hours(props: { flight: Flight; width: number; left: number }) {
  const host = props.flight.host;
  const key = (props.width - 28 - 16) / 3;
  return (
    <View class="flex-col" style={{ width: props.width }}>
      <View style={{ paddingT: 8 }}><TimeBar flight={props.flight} width={props.width} left={props.left} /></View>
      <View class="flex-row flex-wrap gap-2" style={{ paddingL: 14, paddingT: 6, width: props.width }}>
        <For each={HOURS}>
          {([label, minutes]) => <Button label={`${label} ${clock(minutes)}`} width={key} height={40} onPress={() => host.send({ type: "hour", minutes })} />}
        </For>
      </View>
      <Show when={props.flight.flow()}>
        {(flow) => (
          <View class="flex-row items-center gap-2" style={{ paddingL: 14, paddingT: 10 }}>
            <Text class="text-xs font-bold tracking-wide" style={{ width: 56, textColor: DIM }}>CLOCK</Text>
            <For each={flow().choices ?? []}>
              {(choice, index) => <Button label={choice} width={(props.width - 28 - 56 - 24) / 3} height={36} strong={flow().value === index()} onPress={() => props.flight.set(flow(), index())} />}
            </For>
          </View>
        )}
      </Show>
    </View>
  );
}

/** The list that is up: its heading with the way back, and its rows; the hours are keys. */
function Sheet(props: { flight: Flight; menu: Menu; width: number; left: number; active: () => boolean; children?: JSX.Element }) {
  const flight = props.flight;
  return (
    <View class="flex-col">
      <Heading text={flight.heading()} width={props.width} height={TARGET} back={flight.sheet() !== "menu" ? "Back" : flight.mode() === "menu" ? "Resume" : undefined} onBack={flight.back} />
      {props.children}
      <Show when={flight.sheet() === "time"} fallback={<Rows flight={flight} menu={props.menu} width={props.width} rowHeight={TARGET} infoHeight={30} active={props.active} />}>
        <Hours flight={flight} width={props.width} left={props.left} />
      </Show>
    </View>
  );
}

function Title(props: { flight: Flight; menu: Menu }) {
  const flight = props.flight;
  const host = flight.host;
  return (
    <View class="relative w-full h-full">
      <View class="absolute bg-gradient-to-r from-[#000000c0] to-[#00000000]" style={{ insetL: 0, insetT: 0, width: 340, height: H }} />
      <View class="absolute" style={{ insetL: 28, insetT: 26 }}><Wordmark large /></View>
      <Text class="absolute text-xs" style={{ insetL: 28, insetT: 92, textColor: DIM }}>A flight over Shiba, around Tokyo Tower.</Text>
      <View class="absolute" style={{ insetR: 16, insetT: 14 }}><Clock host={host} /></View>
      <Show
        when={flight.sheet() === "menu"}
        fallback={<Panel width={W} height={H} panelWidth={380} panelHeight={296}><Sheet flight={flight} menu={props.menu} width={380} left={(W - 380) / 2} active={() => flight.mode() === "title"} /></Panel>}
      >
        <View class="absolute flex-col gap-2" style={{ insetL: 28, insetT: 124 }}>
          <Button label="Take the tour" width={212} height={TARGET} strong onPress={() => host.send({ type: "start", tour: true })} />
          <Button label="Fly yourself" width={212} height={TARGET} onPress={() => host.send({ type: "start", tour: false })} />
        </View>
        <View class="absolute flex-row gap-2" style={{ insetL: 28, insetB: 20 }}>
          <Button label="Time of day" width={104} height={40} onPress={() => flight.open("time")} />
          <Button label="Settings" width={88} height={40} onPress={() => flight.open("settings")} />
          <Button label="About" width={72} height={40} onPress={() => flight.open("about")} />
        </View>
      </Show>
    </View>
  );
}

const MAP = 196;

/** The menu over a flight: the area from above beside the list. The hours take the panel's whole width. */
function Over(props: { flight: Flight; menu: Menu }) {
  const side = MAP + 20, panel = 460;
  const wide = () => props.flight.sheet() === "time";
  return (
    <Panel width={W} height={H} panelWidth={panel} panelHeight={296}>
      <View class="items-center justify-center flex-col gap-3" style={{ width: side, height: 296, display: wide() ? 1 : 0 }}>
        <AreaMap host={props.flight.host} width={MAP} />
        <Text class="text-sm font-bold" style={{ textColor: INK }}>{props.flight.place()}</Text>
      </View>
      <Show when={wide()} fallback={<Sheet flight={props.flight} menu={props.menu} width={panel - side} left={(W - panel) / 2 + side} active={() => props.flight.mode() === "menu"} />}>
        <Sheet flight={props.flight} menu={props.menu} width={panel} left={(W - panel) / 2} active={() => props.flight.mode() === "menu"} />
      </Show>
    </Panel>
  );
}

/** A stick's ring, how far around its centre a thumb still takes hold of it,
 *  and the play at its centre that does nothing. */
const STICK = 46, KNOB = 44, REACH = 84, SLACK = 0.18;

/** A stick that stays where it is drawn: a thumb landing on or near it
 *  pushes it toward where it landed. */
function Stick(props: { at: { x: number; y: number }; onChange: (x: number, y: number) => void }) {
  let area: NodeMirror | undefined;
  const [knob, setKnob] = createSignal({ x: 0, y: 0 });
  const [held, setHeld] = createSignal(false);
  const push = (c: { x: number; y: number }) => {
    let x = (c.x - props.at.x) / STICK, y = (c.y - props.at.y) / STICK;
    const length = Math.hypot(x, y);
    if (length > 1) {
      x /= length;
      y /= length;
    }
    setKnob({ x, y });
    // Past the slack the push grows from nothing, so the first step is a small one.
    const drive = length <= SLACK ? 0 : (Math.min(1, length) - SLACK) / (1 - SLACK) / Math.min(1, length);
    props.onChange(x * drive, y * drive);
  };
  const rest = () => {
    setHeld(false);
    setKnob({ x: 0, y: 0 });
    props.onChange(0, 0);
  };
  createGesture({
    region: { node: () => area },
    tapSlop: 9999,
    onDown: (c) => {
      setHeld(true);
      push(c);
    },
    onMove: push,
    onUp: rest,
    onCancel: rest,
  });
  const alpha = () => (held() ? 0.5 : 0.26);
  return (
    <View ref={area} class="absolute" style={{ insetL: props.at.x - REACH, insetT: props.at.y - REACH, width: REACH * 2, height: REACH * 2 }}>
      <View class="absolute rounded-full w-[92] h-[92]" style={{ insetL: REACH - STICK, insetT: REACH - STICK, bgColor: tint("#ffffff", alpha() * 0.35), borderWidth: 2, borderColor: tint("#ffffff", alpha()) }} />
      <View class="absolute rounded-full w-[44] h-[44]" style={{ insetL: REACH - KNOB / 2, insetT: REACH - KNOB / 2, translateX: knob().x * STICK, translateY: knob().y * STICK, bgColor: tint("#ffffff", alpha() + 0.15) }} />
    </View>
  );
}

/** A round key under a thumb, held while a finger is on it. */
function Key(props: { label: string; x: number; y: number; size: number; onChange: (held: boolean) => void }) {
  let node: NodeMirror | undefined;
  const [held, setHeld] = createSignal(false);
  const set = (on: boolean) => {
    setHeld(on);
    props.onChange(on);
  };
  createGesture({
    region: { node: () => node },
    tapSlop: 9999,
    onDown: () => set(true),
    onUp: () => set(false),
    onCancel: () => set(false),
  });
  return (
    <View ref={node} class="absolute items-center justify-center" style={{ insetL: props.x - props.size / 2, insetT: props.y - props.size / 2, width: props.size, height: props.size, radius: props.size / 2, bgColor: held() ? tint(TOWER, 0.85) : "#0b0f1580", borderWidth: 2, borderColor: held() ? TOWER : "#ffffff70" }}>
      <Text class="text-xs font-bold" style={{ textColor: held() ? NIGHT : INK }}>{props.label}</Text>
    </View>
  );
}

function Controls(props: { flight: Flight }) {
  const host: Host = props.flight.host;
  // The stick and the keys, sent when they change; the stick's y is forward.
  const stick = { x: 0, y: 0 };
  let keys = 0, sent = "0,0,0";
  const key = (bit: number) => (on: boolean) => (keys = on ? keys | bit : keys & ~bit);
  onFrame(() => {
    const now = [Math.round(stick.x * 100), Math.round(-stick.y * 100), keys];
    const line = now.join(",");
    if (line === sent) return;
    sent = line;
    host.send({ type: "drive", mx: now[0], my: now[1], b: keys });
  });
  // A finger on the city turns the view by what it travels.
  let scene: NodeMirror | undefined;
  createGesture({
    region: { node: () => scene },
    tapSlop: 9999,
    onMove: (c) => {
      if (c.fdx || c.fdy) host.send({ type: "look", dx: c.fdx, dy: c.fdy });
    },
  });
  const [hint, showHint] = createPulse(host, 7);
  createEffect(on(host.mode, (mode, before) => mode === "flight" && before === "title" && showHint()));
  // The day as a bar, under the clock while the clock was tapped.
  const [timing, setTiming] = createSignal(false);
  createEffect(on(host.mode, () => setTiming(false)));
  const BAR = 300;
  return (
    <View class="relative w-full h-full">
      {/* The instruments are children of the scene: a finger that lands on one still turns the view. */}
      <View ref={scene} class="absolute" style={{ insetL: 0, insetT: 0, width: W, height: H }}>
        <View class="absolute" style={{ insetL: 68, insetT: 12 }}><Place flight={props.flight} width={180} /></View>
        <View class="absolute" style={{ insetL: W / 2 - 60, insetB: 10 }}><Compass host={host} width={120} /></View>
        <View class="absolute" style={{ insetL: W / 2 - 60, insetB: 58 }}><Readout host={host} of="altitude" compact /></View>
        <View class="absolute" style={{ insetL: W / 2 + 8, insetB: 58 }}><Readout host={host} of="speed" compact /></View>
        <View class="absolute" style={{ insetL: 0, insetT: 84 }}><Note flight={props.flight} width={W} /></View>
        <View class="absolute" style={{ insetL: 68, insetT: 60 }}><Stats host={host} /></View>
        <View class="absolute items-center justify-center" style={{ insetL: 0, insetT: 120, width: W }}>
          <Fade shown={hint()}><Chip text="Drag to look · tap the clock to turn the hour" /></Fade>
        </View>
      </View>
      <Touchable class="absolute rounded-lg items-center justify-center flex-col gap-1" style={{ insetL: 10, insetT: 10, width: TARGET, height: TARGET, bgColor: "#0b0f15b0", borderWidth: 1, borderColor: HAIRLINE }} onTap={props.flight.menu}>
        <View style={{ width: 18, height: 2, bgColor: INK }} />
        <View style={{ width: 18, height: 2, bgColor: INK }} />
        <View style={{ width: 18, height: 2, bgColor: INK }} />
      </Touchable>
      <Touchable class="absolute" style={{ insetR: 16, insetT: 12, width: 76, height: 46 }} onTap={() => setTiming(!timing())}>
        <Clock host={host} />
      </Touchable>
      <View class="absolute" style={{ insetR: 106, insetT: 12 }}>
        <Button label={host.tour() ? "TOUR ON" : "TOUR"} width={84} height={TARGET} strong={host.tour()} onPress={() => host.send({ type: "tour", on: !host.tour() })} />
      </View>
      <View class="absolute rounded-lg" style={{ insetL: (W - BAR) / 2, insetT: 66, width: BAR, height: 40, bgColor: GLASS, borderWidth: 1, borderColor: HAIRLINE, display: timing() ? 0 : 1 }}>
        <TimeBar flight={props.flight} width={BAR} left={(W - BAR) / 2} />
      </View>
      <Stick at={{ x: 86, y: H - 86 }} onChange={(x, y) => { stick.x = x; stick.y = y; }} />
      <Key label="UP" x={W - 56} y={H - 150} size={64} onChange={key(FLY.up)} />
      <Key label="DOWN" x={W - 56} y={H - 68} size={64} onChange={key(FLY.down)} />
      <Key label="FAST" x={W - 134} y={H - 62} size={56} onChange={key(FLY.fast)} />
    </View>
  );
}
