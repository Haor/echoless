export type AutostartPreference = {
  enabled: boolean | null;
  pending: boolean | null;
};

export function displayAutostartEnabled(state: AutostartPreference): boolean {
  return state.pending ?? state.enabled ?? false;
}

export function beginAutostartChange(
  state: AutostartPreference,
  enabled: boolean,
): AutostartPreference {
  return { enabled: state.enabled, pending: enabled };
}

export function settleAutostart(enabled: boolean): AutostartPreference {
  return { enabled, pending: null };
}

export function rejectAutostartChange(
  state: AutostartPreference,
): AutostartPreference {
  return { enabled: state.enabled, pending: null };
}
