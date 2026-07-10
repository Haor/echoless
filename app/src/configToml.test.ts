import { describe, expect, it } from "vitest";
import { buildConfigToml, tomlString } from "./api";

describe("TOML basic string encoding", () => {
  it.each([
    ["line feed", "\n", '"\\n"'],
    ["carriage return", "\r", '"\\r"'],
    ["backspace", "\b", '"\\b"'],
    ["form feed", "\f", '"\\f"'],
    ["tab", "\t", '"\\t"'],
    ["NUL", "\0", '"\\u0000"'],
    ["escape", "\x1b", '"\\u001B"'],
    ["delete", "\x7f", '"\\u007F"'],
    ["quote", '"', '"\\\""'],
    ["backslash", "\\", '"\\\\"'],
    ["Unicode", "雪😀", '"雪😀"'],
  ])("encodes %s", (_name, input, expected) => {
    expect(tomlString(input)).toBe(expected);
  });

  it("never emits a raw C0 control or DEL", () => {
    const controls = String.fromCharCode(...Array.from({ length: 32 }, (_, i) => i), 0x7f);

    const encoded = tomlString(controls);

    expect([...encoded].some((char) => {
      const codePoint = char.codePointAt(0)!;
      return codePoint <= 0x1f || codePoint === 0x7f;
    })).toBe(false);
  });

  it("keeps hostile selectors inside one valid string value", () => {
    const selector = `mic\ncr\rback\bform\fnull\0esc\x1bdel\x7f"\\雪😀`;

    const config = buildConfigToml({
      mic: selector,
      reference: "none",
      output: "default",
      kind: "passthrough",
      pipeline: {
        sample_rate: 48_000,
        frame_ms: 10,
        reference_channels: "mono",
      },
      params: {},
    });

    expect(config).toContain(
      'mic = "mic\\ncr\\rback\\bform\\fnull\\u0000esc\\u001Bdel\\u007F\\"\\\\雪😀"',
    );
    expect(config.split("\n", 2)[0]).toContain("\\ncr\\r");
  });
});
