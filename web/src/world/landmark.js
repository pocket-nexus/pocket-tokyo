// Landmarks. A landmark is a building the mesher builds from a model of its own, member by member (tower.js
// is one such model), where the city's data has only its outline. Besides the quads the reference draws, a
// model reports what it is made of: beams, boxes, lamps. An export collects that here (src/pocket/export.js),
// and a city compiler makes a model of the landmark for each machine from the members themselves: all of
// them where the machine can draw them, the main ones farther off.
//
// A model for another landmark reports the same way:
//
//   member({ kind: 'beam', p, q, thick, colour, rank })   a straight member from p to q; rank 0 for the ones
//                                                          that carry the outline, 1 for the frame, 2 for bracing
//   member({ kind: 'box', ring, y0, y1, colour })          a closed prism over four corners [x, z]
//   member({ kind: 'lamp', at, size, colour })
//
// Colours are linear RGB.
export const landmarks = { sink: null };

// What a model calls for each member; null when nobody collects.
export function reporter(head) {
  if (!landmarks.sink) return null;
  const record = { ...head, members: [] };
  landmarks.sink.push(record);
  return (m) => record.members.push(m);
}
