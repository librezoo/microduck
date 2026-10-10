// @vitest-environment jsdom
// Drives the REAL LanAudioAdapter (the app's own code) against a running showd.
// SHOWD_ADDRESS=127.0.0.1:8767 SHOWD_CODE=SIMCODE1 pnpm vitest run src/device/showd.live.test.ts
import { describe, expect, it } from "vitest";
import { LanAudioAdapter, type DeviceSnapshot } from "./adapter";

const address = process.env.SHOWD_ADDRESS;
const code = process.env.SHOWD_CODE ?? "";
const live = address ? describe : describe.skip;

function wav(seconds: number, rate = 48000, channels = 2): Blob {
  const frames = seconds * rate;
  const data = new Int16Array(frames * channels);
  for (let i = 0; i < frames; i++) for (let c = 0; c < channels; c++) data[i * channels + c] = Math.round(2500 * Math.sin((2 * Math.PI * 330 * i) / rate));
  const head = new DataView(new ArrayBuffer(44));
  const w = (o: number, s: string) => [...s].forEach((ch, k) => head.setUint8(o + k, ch.charCodeAt(0)));
  w(0, "RIFF"); head.setUint32(4, 36 + data.byteLength, true); w(8, "WAVE"); w(12, "fmt ");
  head.setUint32(16, 16, true); head.setUint16(20, 1, true); head.setUint16(22, channels, true);
  head.setUint32(24, rate, true); head.setUint32(28, rate * channels * 2, true); head.setUint16(32, channels * 2, true);
  head.setUint16(34, 16, true); w(36, "data"); head.setUint32(40, data.byteLength, true);
  return new Blob([head, data]);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

live("the app's LanAudioAdapter against showd", () => {
  it("pairs, uploads, plays, pauses, seeks, sets the volume and stops", async () => {
    const connection = await LanAudioAdapter.pair(address!, code);
    expect(connection.deviceId).toMatch(/microduck/);
    const adapter = new LanAudioAdapter(connection);
    await adapter.checkIdentity();

    let snap: DeviceSnapshot = await adapter.snapshot();
    expect(snap.protocol).toBe(1);
    const progress: number[] = [];
    snap = await adapter.upload({ id: "studio-1", title: "来自 Studio", voice: "ah" }, wav(6), "", (p) => progress.push(p));
    const song = snap.songs.find((s) => s.id === "studio-1")!;
    expect(song.title).toBe("来自 Studio");
    expect(song.duration).toBeGreaterThan(5.9);

    snap = await adapter.control({ action: "play", songId: song.id, revision: song.revision }, snap);
    await sleep(2500);
    snap = await adapter.snapshot();
    expect(["playing", "paused"]).toContain(snap.playback.status);
    expect(snap.playback.songId).toBe("studio-1");

    snap = await adapter.control({ action: "pause" }, snap);
    expect(snap.playback.status).toBe("paused");
    snap = await adapter.control({ action: "resume" }, snap);
    snap = await adapter.control({ action: "seek", position: 3 }, snap);
    snap = await adapter.control({ action: "volume", volume: 40 }, snap);
    expect(snap.volume).toBe(40);

    snap = await adapter.control({ action: "stop" }, snap);
    await sleep(800);
    snap = await adapter.snapshot();
    expect(snap.playback.status).toBe("stopped");

    // A stale revision is refused in a sentence a person can read.
    await expect(adapter.control({ action: "play", songId: song.id, revision: "old" }, snap)).rejects.toThrow(/版本|更新|同步/);
    adapter.dispose();
  }, 60000);
});
