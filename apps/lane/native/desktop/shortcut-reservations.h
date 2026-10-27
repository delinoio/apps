// SPDX-License-Identifier: Apache-2.0
#pragma once

// Capture registration supports letters and digits. Reserve every tab chord
// representable by that grammar before replacing an existing binding.
inline bool LaneReservedCaptureShortcut(unsigned modifiers, char key,
                                        unsigned primary, unsigned shift) {
  if (modifiers == primary) {
    if (key == 'Q' || key == 'O' || key == 'K' || key == 'T' || key == 'W')
      return true;
    if (key >= '1' && key <= '9') return true;
  }
  return modifiers == (primary | shift) && key == 'T';
}
