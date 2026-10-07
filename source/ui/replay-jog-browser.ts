import fs from 'node:fs';
import {PlaybackClock} from './src/playbackClock';
import {liveDeck} from './src/useLiveDecks';
import type {LivePlayer} from './src/model';
const data=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
const snapshots=data.browser.snapshots as {browserMs:number;decks:LivePlayer[]}[];
const clocks=new Map<string,PlaybackClock>();
const results=new Map<string,{frames:number;corrections:{at:number;delta:number}[]}>();
for(const frame of data.browser.frames){
 const candidates=snapshots.filter(s=>s.browserMs<=frame.browserMs).flatMap(s=>s.decks).filter(p=>p.trackKey===frame.trackKey&&p.observationId===frame.observationId);
 const player=candidates.at(-1);if(!player)continue;
 const motion=liveDeck(player,undefined,frame.browserMs-frame.receiptToFrameMs,frame.browserMs-frame.estimatedObservationAgeMs)?.motion;if(!motion)continue;
 let clock=clocks.get(frame.trackKey);if(!clock){clock=new PlaybackClock();clocks.set(frame.trackKey,clock);results.set(frame.trackKey,{frames:0,corrections:[]});}
 const result=results.get(frame.trackKey)!;
 const before=clock.read(frame.browserMs);clock.update(motion);const after=clock.read(frame.browserMs);
 if(result.frames>0&&Math.abs(after-before)>.01)result.corrections.push({at:frame.browserMs,delta:after-before});
 result.frames++;
}
console.log(JSON.stringify({assumption:'Uses recorded browser observations and callback times; no physical latency/accuracy ground truth.',results:Object.fromEntries(results)},null,2));
