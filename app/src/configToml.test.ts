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

  it("preserves a Windows WASAPI endpoint selector with braces", () => {
    const selector =
      "{0.0.1.00000000}.{8f6f0d71-4c2b-4a57-bfd3-5f47cdbb6f50}";

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

    expect(config).toBe(
      [
        `mic = "${selector}"`,
        'reference = "none"',
        'output = "default"',
        "sample_rate = 48000",
        "frame_ms = 10",
        'reference_channels = "mono"',
        "output_level = 50",
        "",
        "[[chain]]",
        'kind = "passthrough"',
        "",
      ].join("\n"),
    );
  });

  it("does not serialize the removed NVAFX runtime override", () => {
    const config = buildConfigToml({
      mic: "default",
      reference: "system",
      output: "default",
      kind: "nvidia_afx_aec",
      pipeline: {
        sample_rate: 48_000,
        frame_ms: 10,
        reference_channels: "mono",
      },
      params: {
        runtime_dir: "C:\\outside\\fixed-root",
        intensity_ratio: 0.8,
      },
    });

    expect(config).not.toContain("runtime_dir");
    expect(config).toContain("intensity_ratio = 0.8");
  });

  it("serializes diagnostics without a custom directory", () => {
    const config = buildConfigToml({
      mic: "default",
      reference: "system",
      output: "default",
      kind: "aec3",
      pipeline: {
        sample_rate: 48_000,
        frame_ms: 10,
        reference_channels: "mono",
      },
      params: {},
      diagnostics: { max_seconds: 30 },
    });

    expect(config).toContain("[diagnostics]\nmax_seconds = 30");
    expect(config).not.toContain("record_dir");
  });
});
