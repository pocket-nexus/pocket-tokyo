// Two screens (the 3DS): the city and its instruments on the top screen, the
// area from above on the touch screen below, with the clock as a bar a
// stylus turns. A list (the title, the menu, the settings) takes the lower
// screen, where a finger or the d-pad walks it.
import { createEffect, on, Show } from "solid-js";
import { AuxiliarySurface, Text, View } from "@pocketjs/framework/components";
import { glyph } from "@pocketjs/framework/modality";
import { createFlight, createPulse, type Flight } from "../flight.ts";
import { AREA } from "../generated/area.ts";
import { connectHost } from "../host.ts";
import { AreaMap, Button, Chip, Clock, Compass, createMenu, Fade, Heading, Keep, Legend, Loading, type Menu, Note, Place, Readout, Rows, Stats, TimeBar, Wordmark } from "../parts.tsx";
import { DIM, HAIRLINE, NIGHT } from "../theme.ts";

const TOP = { w: 400, h: 240 };
const LOW = { w: 320, h: 240 };
const BAR = 30;
/** The map's width on the lower screen: its picture, one sample per pixel. */
const MAP = 255;

export default function DualScreen() {
  const host = connectHost();
  const flight = createFlight(host, false);
  const menu = createMenu(flight);
  return (
    <>
      <View class="relative" style={{ width: TOP.w, height: TOP.h }}>
        <Show when={host.mode() === "loading" || host.mode() === "error"}><Loading host={host} width={TOP.w} height={TOP.h} /></Show>
        <Keep when={host.mode() === "title"} eager width={TOP.w} height={TOP.h}><TitleTop flight={flight} /></Keep>
        <Keep when={host.mode() === "flight" || host.mode() === "menu"} eager width={TOP.w} height={TOP.h}><InstrumentsTop flight={flight} /></Keep>
      </View>
      <AuxiliarySurface>
        {() => (
          <View class="relative" style={{ width: LOW.w, height: LOW.h, bgColor: NIGHT }}>
            <Show when={host.mode() === "loading" || host.mode() === "error"}>
              <View class="items-center justify-center" style={{ width: LOW.w, height: LOW.h }}>
                <Text class="text-sm" style={{ textColor: DIM }}>{host.mode() === "error" ? "The city could not be read" : "Loading"}</Text>
              </View>
            </Show>
            {/* The map stays built under the lists: opening one and closing it build nothing. */}
            <Keep when={host.mode() === "flight"} eager width={LOW.w} height={LOW.h}><FlightLow flight={flight} /></Keep>
            <Keep when={flight.listing()} eager width={LOW.w} height={LOW.h}><ListLow flight={flight} menu={menu} /></Keep>
          </View>
        )}
      </AuxiliarySurface>
    </>
  );
}

function TitleTop(props: { flight: Flight }) {
  return (
    <View class="relative" style={{ width: TOP.w, height: TOP.h }}>
      <View class="absolute bg-gradient-to-b from-[#00000000] to-[#000000c0]" style={{ insetL: 0, insetB: 0, width: TOP.w, height: 130 }} />
      <View class="absolute" style={{ insetL: 20, insetB: 34 }}><Wordmark large /></View>
      <Text class="absolute text-xs" style={{ insetL: 20, insetB: 14, textColor: DIM }}>A flight over Shiba, around Tokyo Tower.</Text>
      <View class="absolute" style={{ insetR: 14, insetT: 12 }}><Clock host={props.flight.host} /></View>
    </View>
  );
}

function InstrumentsTop(props: { flight: Flight }) {
  const host = props.flight.host;
  const [hint, showHint] = createPulse(7);
  createEffect(on(host.mode, (mode, before) => mode === "flight" && before === "title" && showHint()));
  return (
    <View class="relative" style={{ width: TOP.w, height: TOP.h }}>
      <View class="absolute" style={{ insetL: 16, insetT: 12 }}><Place flight={props.flight} width={180} /></View>
      <View class="absolute" style={{ insetR: 14, insetT: 12 }}><Clock host={host} /></View>
      <View class="absolute" style={{ insetL: 16, insetB: 12 }}><Readout host={host} of="altitude" /></View>
      <View class="absolute" style={{ insetL: TOP.w / 2 - 60, insetB: 12 }}><Compass host={host} width={120} /></View>
      <View class="absolute" style={{ insetR: 14, insetB: 12 }}><Readout host={host} of="speed" /></View>
      <View class="absolute" style={{ insetL: 0, insetT: 70 }}><Note flight={props.flight} width={TOP.w} /></View>
      <View class="absolute" style={{ insetL: 10, insetT: 60 }}><Stats host={host} /></View>
      <View class="absolute items-center justify-center" style={{ insetL: 0, insetB: 62, width: TOP.w }}>
        <Fade shown={hint()}><Chip text={`${glyph("select")} tour · D-pad left/right clock · ${glyph("start")} menu`} /></Fade>
      </View>
    </View>
  );
}

const SIDE = LOW.w - MAP;

/** During a flight: the area at the left, the menu and the tour beside it, the day along the bottom. */
function FlightLow(props: { flight: Flight }) {
  const host = props.flight.host;
  return (
    <View class="relative" style={{ width: LOW.w, height: LOW.h }}>
      <AreaMap host={host} width={MAP} />
      <View class="absolute" style={{ insetL: MAP, insetT: 0, width: 1, height: LOW.h - 36, bgColor: HAIRLINE }} />
      <View class="absolute flex-col gap-2" style={{ insetL: MAP + 5, insetT: 6 }}>
        <Button label="Menu" width={SIDE - 9} height={38} surface="auxiliary" onPress={props.flight.menu} />
        <Button label="Tour" width={SIDE - 9} height={38} strong={host.tour()} surface="auxiliary" onPress={() => host.send({ type: "tour", on: !host.tour() })} />
      </View>
      <View class="absolute" style={{ insetL: MAP + 7, insetB: 46 }}><Clock host={host} compact /></View>
      <View class="absolute" style={{ insetL: 0, insetB: 36, width: LOW.w, height: 1, bgColor: HAIRLINE }} />
      <View class="absolute" style={{ insetL: 0, insetB: 0 }}><TimeBar flight={props.flight} width={LOW.w} left={0} surface="auxiliary" /></View>
    </View>
  );
}

/** A list on the lower screen: its heading, its rows, what the buttons do. */
function ListLow(props: { flight: Flight; menu: Menu }) {
  const flight = props.flight;
  return (
    <View class="relative flex-col" style={{ width: LOW.w, height: LOW.h, bgColor: NIGHT }}>
      <View style={{ width: LOW.w, height: BAR, bgColor: "#141922" }}>
        <Heading text={flight.heading() || `${AREA.name.toUpperCase()}, ${AREA.district.toUpperCase()}`} width={LOW.w} height={BAR} back={flight.sheet() !== "menu" ? "Back" : undefined} onBack={flight.back} surface="auxiliary" />
      </View>
      <Rows flight={flight} menu={props.menu} width={LOW.w} rowHeight={flight.sheet() === "time" ? 26 : 30} surface="auxiliary" />
      <View class="absolute" style={{ insetL: 0, insetB: 0 }}>
        <Legend width={LOW.w} legend={props.menu.actions.legend()} />
      </View>
    </View>
  );
}
