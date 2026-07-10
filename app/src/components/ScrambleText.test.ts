import { describe, expect, it } from "vitest";
import { writeSafeText } from "./ScrambleText";

const hostileText = [
  `<>&"'`,
  "<style>body{display:none}</style>",
  '<meta http-equiv="refresh" content="0;url=https://example.invalid">',
];

describe("ScrambleText", () => {
  it.each(hostileText)("preserves hostile input as text: %s", (text) => {
    let written: string | null = null;
    const sink = {
      get textContent() {
        return written;
      },
      set textContent(value: string | null) {
        written = value;
      },
      set innerHTML(_value: string) {
        throw new Error("hostile text reached an HTML-writing sink");
      },
    };

    writeSafeText(sink, text);

    expect(written).toBe(text);
  });
});
