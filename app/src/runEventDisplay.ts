import type { ControlErrorEvent, StreamErrorEvent } from "./types";

export function controlErrorMessage(event: ControlErrorEvent): string {
  const command = event.cmd?.trim() || "runtime control";
  return `${command}: ${event.message}`;
}

export function streamErrorMessage(event: StreamErrorEvent): string {
  return `${event.stream} stream error: ${event.message}`;
}
