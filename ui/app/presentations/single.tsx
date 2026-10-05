// One 480×272 screen with a pad: the PSP, and the Vita, whose panel also
// takes taps. During a flight the city has the screen and the interface is
// the instruments at its edges; a list comes up over it for the title and
// the menu.
import { createEffect, on, Show } from "solid-js";
import { Text, View } from "@pocketjs/framework/components";
import { glyph } from "@pocketjs/framework/modality";
import { createFlight, createPulse, type Flight } from "../flight.ts";
import { AREA } from "../generated/area.ts";
import { connectHost } from "../host.ts";
import { AreaMap, Chip, Clock, Compass, createMenu, Fade, Heading, Keep, Legend, Loading, type Menu, Note, Panel, Place, Readout, Rows, Stats, Wordmark } from "../parts.tsx";
import { DIM } from "../theme.ts";

const W = 480, H = 272;
const FOOTER = 24;

/** A list's rows: the hours stand closer together, so that the day fits a panel. */
const rowHeight = (flight: Flight) => (flight.sheet() === "time" ? 26 : 30);

export default function SingleScreen() {
  const host = connectHost();
  const flight = createFlight(host, false);
  const menu = createMenu(flight);
  return (
    <View class="relative w-full h-full">
      <Show when={host.mode() === "loading" || host.mode() === "error"}><Loading host={host} width={W} height={H} /></Show>
      <Keep when={host.mode() === "title"} eager width={W} height={H}><Title flight={flight} menu={menu} /></Keep>
      <Keep when={host.mode() === "flight"} eager width={W} height={H}><Instruments flight={flight} /></Keep>
      <Keep when={host.mode() === "menu"} eager width={W} height={H}><Over flight={flight} menu={menu} /></Keep>
      <Show when={flight.listing()}>
        <View class="absolute" style={{ insetL: 0, insetB: 0 }}>
          <Legend width={W} left={flight.mode() === "title" ? `${AREA.name}, ${AREA.district}` : flight.place()} legend={menu.actions.legend()} />
        </View>
      </Show>
    </View>
  );
}

function Title(props: { flight: Flight; menu: Menu }) {
  const flight = props.flight;
  return (
    <View class="relative w-full h-full">
      <View class="absolute bg-gradient-to-r from-[#000000c0] to-[#00000000]" style={{ insetL: 0, insetT: 0, width: 320, height: H }} />
      <View class="absolute" style={{ insetL: 24, insetT: 22 }}><Wordmark large /></View>
      <Text class="absolute text-xs" style={{ insetL: 24, insetT: 88, textColor: DIM }}>A flight over Shiba, around Tokyo Tower.</Text>
      <View class="absolute" style={{ insetR: 16, insetT: 12 }}><Clock host={flight.host} /></View>
      <Show
        when={flight.sheet() === "menu"}
        fallback={
          <Panel width={W} height={H - FOOTER} panelWidth={320} panelHeight={216}>
            <View class="flex-col">
              <Heading text={flight.heading()} width={320} height={28} />
              <Rows flight={flight} menu={props.menu} width={320} rowHeight={rowHeight(flight)} active={() => flight.mode() === "title"} />
            </View>
          </Panel>
        }
      >
        <View class="absolute" style={{ insetL: 10, insetT: 110 }}><Rows flight={flight} menu={props.menu} width={190} rowHeight={26} active={() => flight.mode() === "title"} /></View>
      </Show>
    </View>
  );
}

/** The flight's instruments: the place and the clock above, the height, the heading and the speed below. */
function Instruments(props: { flight: Flight }) {
  const host = props.flight.host;
  // What the buttons do shows when a flight begins, then leaves the view clear.
  const [hint, showHint] = createPulse(7);
  createEffect(on(host.mode, (mode, before) => mode === "flight" && before === "title" && showHint()));
  return (
    <View class="relative w-full h-full">
      <View class="absolute" style={{ insetL: 18, insetT: 12 }}><Place flight={props.flight} width={190} /></View>
      <View class="absolute" style={{ insetR: 16, insetT: 12 }}><Clock host={host} /></View>
      <View class="absolute" style={{ insetL: 18, insetB: 12 }}><Readout host={host} of="altitude" /></View>
      <View class="absolute" style={{ insetL: W / 2 - 66, insetB: 12 }}><Compass host={host} width={132} /></View>
      <View class="absolute" style={{ insetR: 16, insetB: 12 }}><Readout host={host} of="speed" /></View>
      <View class="absolute" style={{ insetL: 0, insetT: 74 }}><Note flight={props.flight} width={W} /></View>
      <View class="absolute" style={{ insetL: 12, insetT: 62 }}><Stats host={host} /></View>
      <View class="absolute items-center justify-center" style={{ insetL: 0, insetB: 66, width: W }}>
        <Fade shown={hint()}><Chip text={`${glyph("select")} tour · D-pad left/right clock · ${glyph("start")} menu`} /></Fade>
      </View>
    </View>
  );
}

const MAP = 176;

/** The menu over a flight: the area from above beside the list. */
function Over(props: { flight: Flight; menu: Menu }) {
  const list = 440 - MAP - 24;
  return (
    <Panel width={W} height={H - FOOTER} panelWidth={440} panelHeight={216}>
      <View class="items-center justify-center" style={{ width: MAP + 24, height: 216 }}><AreaMap host={props.flight.host} width={MAP} /></View>
      <View class="flex-col">
        <Heading text={props.flight.heading()} width={list} height={28} />
        <Rows flight={props.flight} menu={props.menu} width={list} rowHeight={rowHeight(props.flight)} active={() => props.flight.mode() === "menu"} />
      </View>
    </Panel>
  );
}
