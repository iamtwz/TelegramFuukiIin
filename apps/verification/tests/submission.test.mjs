import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import { readLaunch, encodeSubmission } from '../../../packages/verification-protocol/src/index.ts';
const source = (await readFile(new URL('../public/app.js', import.meta.url), 'utf8')).replace(/^import .*;\n/gm, '');
const fixture = JSON.parse(await readFile(new URL('../../../packages/verification-protocol/fixtures/v2.json', import.meta.url)));
async function page({platform='ios', search=new URLSearchParams(fixture.launch).toString(), failSend=false, failLoad=false}={}) {
  const events = []; const submissions=[]; const widgets=[]; let disposed=0;
  const controls = {};
  const view = { retry:{hidden:true,addEventListener:(_,fn)=>{controls.retry=fn;}}, close:{hidden:true},
    theme:'light',widgetLanguage:'zh-cn',say:key=>events.push(key),busy:()=>{},loading:()=>{},onChange:fn=>{controls.change=fn;} };
  const telegram={platform,ready:()=>{},expand:()=>{},sendData:raw=>{if(failSend) throw new Error('closed');submissions.push(JSON.parse(raw));}};
  const context = vm.createContext({window:{Telegram:{WebApp:telegram},turnstile:{}},location:{search},
    createView:()=>view,loadTurnstile:async()=>{if(failLoad) throw new Error('unavailable');},
    renderChallenge:(_,options)=>{widgets.push(options);return ()=>{disposed++;};},readLaunch,encodeSubmission,
    fetch:()=>{throw new Error('Static page must not call a verification backend');} });
  vm.runInContext(source,context); await new Promise(resolve=>setImmediate(resolve));
  return {events,submissions,widgets,view,controls,get disposed(){return disposed;}};
}
test('keyboard Mini App sends raw token and session through Telegram once, with no backend fetch', async()=>{
  const p=await page();const widget=p.widgets[0];
  assert.equal(widget.sitekey,fixture.launch.sitekey);assert.equal(widget.cData,fixture.launch.session);assert.equal(widget.action,'join');
  widget.callback(fixture.submission.token);widget.callback(fixture.submission.token);
  assert.deepEqual(p.submissions,[fixture.submission]);assert.equal(p.events.at(-1),'sent');assert.ok(p.disposed>0);
});
test('direct browser, old tickets and invalid parameters cannot submit', async()=>{
  for(const options of [{platform:'unknown'}, {search:'ticket=old-proof'}, {search:''}]) {
    const p=await page(options);assert.equal(p.widgets.length,0);assert.equal(p.submissions.length,0);
    assert.ok(['open_in_telegram','invalid_link'].includes(p.events.at(-1)));
  }
});
test('language change invalidates old callbacks and errors offer a fresh challenge', async()=>{
  const p=await page();const old=p.widgets[0];p.controls.change();old.callback('stale-token');assert.equal(p.submissions.length,0);
  p.widgets[1].callback('fresh-token');assert.equal(p.submissions.length,1);
  const failed=await page({failSend:true});failed.widgets[0].callback('valid-token');
  assert.equal(failed.events.at(-1),'return_failed');assert.equal(failed.view.retry.hidden,false);
  await failed.controls.retry();assert.equal(failed.widgets.length,2);
  const loading=await page({failLoad:true});assert.equal(loading.events.at(-1),'captcha_unavailable');assert.equal(loading.view.retry.hidden,false);
});
