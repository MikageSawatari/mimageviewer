// Terminal results belong to an adopted listing, never to evictable DOM cells.
export class AudioArtListState {
  constructor(entries, addressKey, artEpoch, sessionId) {
    this.artEpoch = artEpoch;
    this.sessionId = sessionId;
    this.addressKey = addressKey;
    this.addresses = new Set(entries.filter((entry) => entry.kind === "audio" && !entry.unavailable).map(addressKey));
    this.terminalByAddress = new Map();
  }

  terminal(entry) {
    return this.terminalByAddress.get(this.addressKey(entry));
  }

  settle(entry, result) {
    const key = this.addressKey(entry);
    if (!this.addresses.has(key) || !["NoArt", "Failed"].includes(result)) return false;
    this.terminalByAddress.set(key, result);
    return true;
  }
}

export function audioArtFailure(status, error) {
  if ([401, 403, 409].includes(status) || ["session_required", "session_revoked", "authentication_required"].includes(error)) return null;
  return status === 422 && error === "no_thumbnail" ? "NoArt" : "Failed";
}

export function normalizedAudioIndicator(value) {
  return ["bottom_left_badge", "hidden"].includes(value) ? value : "music_note_icon";
}
