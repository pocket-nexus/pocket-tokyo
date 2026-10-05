// @title Pocket Tokyo
// A touch panel of 16:9 and no buttons: a phone held sideways (the Redmi 1S).
import { mount } from "@pocketjs/framework";
import TouchScreen, { panel } from "./presentations/touch.tsx";

panel(640, 360);
mount(() => <TouchScreen />);
