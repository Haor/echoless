import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { writeSafeText } from "./ScrambleText";

const hostileText = [
  `<>&"'`,
  "<style>body{display:none}</style>",
  '<meta http-equiv="refresh" content="0;url=https://example.invalid">',
];

describe("ScrambleText", () => {
  it.each(hostileText)("preserves hostile input as text: %s", (text) => {
    const sink = { textContent: null as string | null };

    writeSafeText(sink, text);

    expect(sink.textContent).toBe(text);
  });

  it("does not expose an HTML-writing sink", () => {
    const source = readFileSync(
      new URL("./ScrambleText.tsx", import.meta.url),
      "utf8",
    );

    expect(source).not.toMatch(/\binnerHTML\b/);
  });
});
