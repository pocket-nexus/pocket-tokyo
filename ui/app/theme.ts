// The look shared by every device: warm white type over the city, dark
// glass behind anything that must stay legible, and the orange Tokyo Tower
// is painted for what is selected, held or live.
export const INK = "#f6f3ea";
export const DIM = "#f6f3eac4";
export const FAINT = "#f6f3ea80";
export const GLASS = "#0b0f15d9";
export const PLATE = "#0b0f1566";
export const PANEL = "#0e131bf2";
export const HAIRLINE = "#ffffff26";
export const WASH = "#ffffff1c";
export const TOWER = "#ff6a3d";
export const NIGHT = "#090c11";

/** "#rrggbb" with an alpha in 0…1. */
export function tint(color: string, alpha: number): string {
  return color.slice(0, 7) + Math.round(Math.max(0, Math.min(1, alpha)) * 255).toString(16).padStart(2, "0");
}
