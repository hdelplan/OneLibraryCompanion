import fs from 'node:fs';
import {PlaybackClock} from './src/playbackClock';
import {liveDeck} from './src/useLiveDecks';
const data=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
const out:any={decks:{},assumption:'Replay at host publish times with zero extra network/render delay; not original browser timing.'};
for(const number of [1,2]){
 const samples=data.host.samples.filter((s:any)=>s.kind==='published'&&s.deck.number===number);
 const clock=new PlaybackClock();const corrections:any[]=[];let first=true;
 for(const s of samples){const at=s.hostMs;const before=clock.read(at);const motion=liveDeck(s.deck,undefined,at,at-s.deck.statusAgeMs)?.motion;if(!motion)continue;clock.update(motion);const after=clock.read(at);
 if(!first&&Math.abs(after-before)>.01)corrections.push({hostMs:at,deltaSeconds:after-before,state:s.deck.playState,quality:s.deck.positionQuality,manual:s.deck.manualMotion,beat:s.deck.beatNumber});first=false;}
 const steady=corrections.filter(c=>!c.manual&&c.state==='playing');
 out.decks[number]={samples:samples.length,correctionsOver10ms:corrections.length,steadyCorrectionsOver10ms:steady.length,maxSteadyCorrectionSeconds:Math.max(0,...steady.map(c=>Math.abs(c.deltaSeconds))),corrections};
}
console.log(JSON.stringify(out,null,2));
