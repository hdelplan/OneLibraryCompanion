// Feed production host replay output through the production UI motion clock.
// Uses a simulated 60 Hz renderer and known 12 ms delivery delay. This does not
// reproduce the iPad's physical screen or measure network latency.
import fs from "node:fs";
import assert from "node:assert/strict";
import { PlaybackClock } from "./src/playbackClock";
type Sample = {ip:string; hostMs:number; positionMs:number; position:number; rate:number; beatNumber:number};
const data = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const samples = data.samples as Sample[];
const clocks = new Map<string,PlaybackClock>();
const latest = new Map<string,Sample>();
const maxError = new Map<string,number>();
let i = 0, frames = 0;
const delay = 12;
for (let frame = 0; frame <= samples.at(-1)!.hostMs + delay; frame += 1000/60) {
  while (i < samples.length && samples[i].hostMs + delay <= frame) {
    const s = samples[i++];
    const clock = clocks.get(s.ip) ?? new PlaybackClock();
    clocks.set(s.ip,clock);
    clock.update({key:s.ip, position:s.position, rate:s.rate, quality:"beat", playing:true,
      receivedAt:s.hostMs+delay, observedAt:s.positionMs, observationId:String(s.positionMs)});
    latest.set(s.ip,s);
  }
  for (const [ip, clock] of clocks) {
    const s = latest.get(ip)!;
    const expected = s.position + Math.max(0, Math.min(1000,frame-s.positionMs))/1000 * s.rate;
    const error = Math.abs(clock.read(frame)-expected)*1000;
    maxError.set(ip,Math.max(maxError.get(ip)??0,error));
    assert.ok(error < 1e-6, `${ip} unexpected smoothing/delay: ${error} ms`);
    frames++;
  }
}
console.log(JSON.stringify({note:"Simulated renderer/known delivery age; no physical timing claim.", frames, maxExtraDisplayErrorMs:Object.fromEntries(maxError)},null,2));
