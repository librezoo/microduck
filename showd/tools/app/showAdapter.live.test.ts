// @vitest-environment jsdom
// Drives the reference ShowAdapter against a running showd (a duck, or showd on the simulator).
//   SHOWD_ADDRESS=127.0.0.1:8767 SHOWD_CODE=SIMCODE1 pnpm vitest run src/device/showAdapter.live.test.ts
import { Blob as NodeBlob } from "node:buffer"; // jsdom's Blob has no arrayBuffer(); browsers do
import { describe, expect, it } from "vitest";
import { LanAudioAdapter } from "./adapter";
import { ShowAdapter, ShowRefused } from "./showAdapter";

const address = process.env.SHOWD_ADDRESS;
const live = address ? describe : describe.skip;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function wav(seconds: number, rate = 32000): Blob {
  const n = seconds * rate; const d = new Int16Array(n);
  for (let i = 0; i < n; i++) d[i] = Math.round(3000 * Math.sin((2 * Math.PI * 262 * i) / rate));
  const h = new DataView(new ArrayBuffer(44)); const w = (o: number, s: string) => [...s].forEach((c, k) => h.setUint8(o + k, c.charCodeAt(0)));
  w(0, "RIFF"); h.setUint32(4, 36 + d.byteLength, true); w(8, "WAVE"); w(12, "fmt "); h.setUint32(16, 16, true); h.setUint16(20, 1, true); h.setUint16(22, 1, true);
  h.setUint32(24, rate, true); h.setUint32(28, rate * 2, true); h.setUint16(32, 2, true); h.setUint16(34, 16, true); w(36, "data"); h.setUint32(40, d.byteLength, true);
  return new NodeBlob([h, d]) as unknown as Blob;
}

const duck = { schema: "kivi.duck/1", tempoBpm: 96, beatsPerBar: 4, events: [
  { eventId: "n1", startBeat: 0, durationBeats: 1, voice: "ah", midiNote: 62, volume: 0.7 },
  { eventId: "n2", startBeat: 1, durationBeats: 1, voice: "oh", midiNote: 64, volume: 0.75 },
  { eventId: "n3", startBeat: 2, durationBeats: 2, voice: "ah", midiNote: 67, volume: 0.9 }] };
const head = { schema: "kivi.motion/1", actions: [{ eventId: "h", type: "head", startMs: 1000, durationMs: 2000, keyframes: [
  { tMs: 0, neckPitch: 0, headPitch: 0, headYaw: 0, headRoll: 0 }, { tMs: 1000, neckPitch: 0, headPitch: 0, headYaw: 0.3, headRoll: 0 }, { tMs: 2000, neckPitch: 0, headPitch: 0, headYaw: 0, headRoll: 0 }] }] };

live("the reference ShowAdapter against showd", () => {
  it("publishes a show, sends only what changed, is refused with the event at fault, and plays", async () => {
    const conn = await LanAudioAdapter.pair(address!, process.env.SHOWD_CODE ?? "");
    const show = new ShowAdapter(conn.address, conn.token);
    const caps = await show.capabilities();
    expect(caps.audioRoles).toContain("mix");

    const audio = { wav: wav(5), sampleRate: 32000, channels: 1 as const };
    const stored = await show.publish("ref-1", "参考节目", 96, 5000, { duck, motion: head, audio });
    expect(stored.title).toBe("参考节目");

    // Change only the motion: the duck already has the audio and the notes.
    const sent: number[] = [];
    await show.publish("ref-1", "参考节目", 96, 5000, { duck, motion: { ...head, actions: [{ ...head.actions[0], durationMs: 2500 }] }, audio }, undefined, (s) => sent.push(s));
    expect(sent.length).toBe(1);
    expect(sent[0]).toBeLessThan(2000);

    // A skill this duck does not have is refused with the event named.
    const bad = { schema: "kivi.motion/1", actions: [{ eventId: "bow", type: "skill", skill: "stage-bow", startMs: 0, durationMs: 2000 }] };
    const err = await show.publish("ref-2", "坏的", 96, 5000, { duck, motion: bad, audio }).catch((e) => e);
    expect(err).toBeInstanceOf(ShowRefused);
    expect((err as ShowRefused).code).toBe("ActionUnsupported");
    expect((err as ShowRefused).problems[0].eventId).toBe("bow");

    const played = await show.play("ref-1", { mute: false });
    expect(played.accepted).toBe(true);
    await sleep(2500);
    const control = new LanAudioAdapter(conn);
    let snap = await control.snapshot();
    expect(snap.playback.status).toBe("playing");
    snap = await control.control({ action: "stop" }, snap);
  }, 60000);
});
