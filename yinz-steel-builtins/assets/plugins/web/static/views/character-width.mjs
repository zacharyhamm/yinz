import { widthRanges } from './width-ranges.mjs';

function wcwidth(codepoint) {
  if (codepoint >= 0x20 && codepoint <= 0x7e) return 1;
  let low = 0;
  let high = widthRanges.length;
  while (low < high) {
    const mid = (low + high) >>> 1;
    const [start, end, width] = widthRanges[mid];
    if (codepoint < start) high = mid;
    else if (codepoint > end) low = mid + 1;
    else return width;
  }
  return 1;
}

export function useNativeCharacterWidths(term) {
  const version = 'yinz-unicode-width';
  term.unicode.register({
    version,
    wcwidth,
    charProperties(codepoint) {
      // xterm packs width into bits 1–2. The native grid never joins codepoints.
      return wcwidth(codepoint) << 1;
    },
  });
  term.unicode.activeVersion = version;
}
