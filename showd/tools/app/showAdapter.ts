// A reference ShowAdapter: what the app does to hand a multi-track show to a duck.
//
// It sits beside `LanAudioAdapter` (same address, same bearer token, same CORS) and adds the show
// endpoints. It is a reference to port, not a dependency: the app owns its adapter, and this file
// exists so the two sides read the same calls. Everything is fetch + Web Crypto.
//
// Order of a publish: ask what the duck already has -> send only the tracks that differ (each
// with its SHA-256, which the duck checks) -> send the manifest -> commit. Nothing is on the duck
// until `commit` answers; a transfer that stops halfway leaves the library as it was.

export interface DuckNote { eventId: string; startBeat: number; durationBeats: number; voice: "ah" | "eh" | "ee" | "oh" | "oo" | "mm"; midiNote: number; volume: number; continuesPrevious?: boolean }
export interface ShowTracks { duck?: unknown; motion?: unknown; audio?: { wav: Blob; sampleRate: number; channels: 1 | 2; startOffsetMs?: number } }
export interface ShowProblem { code: string; detail: string; eventId?: string }
export class ShowRefused extends Error { constructor(readonly code: string, message: string, readonly problems: ShowProblem[]) { super(message); } }

async function sha256(bytes: ArrayBuffer): Promise<string> {
  const subtle = globalThis.crypto?.subtle ?? (await import("node:crypto")).webcrypto.subtle;
  return [...new Uint8Array(await subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export class ShowAdapter {
  constructor(private readonly origin: string, private readonly token: string) {}

  private async call(method: string, path: string, body?: BodyInit, headers: Record<string, string> = {}) {
    const r = await fetch(`${this.origin}/api/v1${path}`, { method, body, headers: { Authorization: `Bearer ${this.token}`, ...headers }, cache: "no-store" });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) throw new ShowRefused(j.code ?? "Refused", j.error ?? `设备返回 ${r.status}`, j.problems ?? []);
    return j;
  }

  /** What this duck can do right now: skills, limits, pose. Feed it to the motion evaluator. */
  capabilities() { return this.call("GET", "/capabilities"); }

  /** Publish a show. Returns the stored song; throws ShowRefused with the events at fault. */
  async publish(showId: string, title: string, tempoBpm: number, durationMs: number, tracks: ShowTracks, startPose?: "standing" | "sitting", progress?: (sent: number, total: number) => void) {
    const enc = new TextEncoder();
    const parts: Record<string, { bytes: ArrayBuffer; file: string }> = {};
    if (tracks.duck) parts.duck = { file: "duck.score.json", bytes: enc.encode(JSON.stringify(tracks.duck)).buffer as ArrayBuffer };
    if (tracks.motion) parts.motion = { file: "motion.json", bytes: enc.encode(JSON.stringify(tracks.motion)).buffer as ArrayBuffer };
    if (tracks.audio) parts.audio = { file: "audio.wav", bytes: await tracks.audio.wav.arrayBuffer() };
    const manifestTracks: Record<string, unknown> = {};
    const hashes: Record<string, string> = {};
    for (const [name, p] of Object.entries(parts)) {
      hashes[name] = await sha256(p.bytes);
      manifestTracks[name] = name === "audio"
        ? { file: p.file, role: "mix", codec: "pcm_s16le", sampleRate: tracks.audio!.sampleRate, channels: tracks.audio!.channels, startOffsetMs: tracks.audio!.startOffsetMs ?? 0, sha256: hashes[name], bytes: p.bytes.byteLength }
        : { file: p.file, sha256: hashes[name], bytes: p.bytes.byteLength };
    }
    // Only what the duck does not already hold.
    const library = await this.call("GET", "/shows");
    const have: Record<string, string> = {};
    for (const s of library.shows ?? []) if (s.showId === showId) for (const [n, t] of Object.entries<{ sha256: string }>(s.tracks)) have[n] = t.sha256;
    const total = Object.entries(parts).filter(([n]) => have[n] !== hashes[n]).reduce((a, [, p]) => a + p.bytes.byteLength, 0);
    let sent = 0;
    for (const [name, p] of Object.entries(parts)) {
      if (have[name] === hashes[name]) continue;
      await this.call("PUT", `/shows/${showId}/tracks/${name}`, p.bytes, { "Content-Type": "application/octet-stream", "X-Sha256": hashes[name] });
      sent += p.bytes.byteLength;
      progress?.(sent, total);
    }
    const manifest = { schema: "kivi.show/2", showId, title, tempoBpm, durationMs, roles: [{ roleId: "lead", modelId: "microduck-v1" }], ...(startPose ? { startPose } : {}), tracks: manifestTracks };
    await this.call("PUT", `/shows/${showId}/manifest`, JSON.stringify(manifest), { "Content-Type": "application/json" });
    return (await this.call("POST", `/shows/${showId}/commit`, "{}", { "Content-Type": "application/json" })).song;
  }

  play(showId: string, opts: { mute?: boolean; headScale?: number } = {}) { return this.call("POST", `/shows/${showId}/play`, JSON.stringify(opts), { "Content-Type": "application/json" }); }
}
