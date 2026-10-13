import { expect, test } from '@playwright/test';
const modes = {revision:1,active:'super',profiles:[{id:'super',revision:2,name:'Super',kind:'super',instructions:'Frozen polish',text_model:{provider:'gemini',model:'fixture-cleanup'},context:{application:false,selected_text:false,clipboard:false}}]};
for(const integrated of [false,true])test(`batch review, paid gating and versions (${integrated?'desktop':'web'})`,async({page})=>{
  const batches:any[]=[];const mutations:string[]=[];let starts=0;
  await page.route('**/api/v1/**',async route=>{
    const path=new URL(route.request().url()).pathname;const method=route.request().method();if(method!=='GET')mutations.push(path);
    let data:unknown=[];
    if(path==='/api/v1/modes')data=modes;
    else if(path.endsWith('/live-processing'))data={bound:false,attempt:null};
    else if(path.endsWith('/processed'))data={text:''};
    else if(path==='/api/v1/batches'&&method==='POST'){
      const request=route.request().postDataJSON().data;
      data={id:request.id,state:'planned',request,provider_model:'gemini / fixture-cleanup',paid_work:true,likely_paid_items:2,items:request.sources.map((source:any)=>({source,state:source.recording_id==='c'?'unavailable':'queued',unavailable:source.recording_id==='c'?'Audio unavailable; use saved-text cleanup':null,error:null,previous_text:'Earlier result retained',reuse_available:false,version:null}))};batches.unshift(data);
    }else if(path.endsWith('/start')){
      expect(route.request().postDataJSON().data.acknowledge_paid).toBe(true);starts++;
      const batch=batches.find(b=>path.includes(b.id));batch.state='running';batch.items[0].state='succeeded';batch.items[0].version={id:'new-version',source:batch.items[0].source,raw_text:'Original raw שלום',text:'New separate result שלום',reused_from:null};batch.items[1].state='failed';batch.items[1].error='Independent failure';data=batch;
    }else if(path.endsWith('/cancel')){const batch=batches.find(b=>path.includes(b.id));batch.state='canceled';data=batch;}
    else if(path==='/api/v1/batches')data=batches;
    await route.fulfill({json:{api_version:'1',data}});
  });
  await page.goto(`/src/features/home/tests/home.html?integrated=${integrated?'1':'0'}`);
  await page.getByRole('button',{name:'Reprocess saved recordings',exact:true}).click();
  await page.getByRole('combobox',{name:'Reprocessing operation'}).selectOption('cleanup');
  for(const id of ['a','b','c'])await page.getByRole('checkbox',{name:`Select recording ${id}`,exact:true}).check();
  await page.getByRole('button',{name:'Review batch plan (3)'}).click();
  await expect(page.getByText('Audio unavailable; use saved-text cleanup',{exact:true})).toBeVisible();
  const start=page.getByRole('button',{name:'Start reviewed batch'});await expect(start).toBeDisabled();expect(starts).toBe(0);
  await page.getByRole('checkbox',{name:'I understand this batch may incur cloud charges'}).check();await start.click();
  await expect(page.getByText('Independent failure',{exact:true})).toBeVisible();
  await page.getByText('Compare version new-version').click();
  for(const text of ['Earlier result retained','Original raw שלום','New separate result שלום'])await expect(page.getByText(text,{exact:true})).toBeVisible();
  await page.getByRole('button',{name:'Cancel batch',exact:true}).click();await expect(page.getByRole('heading',{name:'Saved-text cleanup · canceled'})).toBeVisible();
  expect(mutations.every(path=>path.startsWith('/api/v1/batches'))).toBe(true);
  await expect(page.getByTestId('transcribed')).toHaveText('');await expect(page.locator('.session-row').first().locator('.row-text')).toContainText('The quick brown fox');
});
test('recovered batches wait for explicit resume across reload',async({page})=>{
  let starts=0;await page.route('**/api/v1/**',route=>{
    const path=new URL(route.request().url()).pathname;let data:unknown=[];if(path==='/api/v1/modes')data=modes;
    if(path==='/api/v1/batches')data=[{id:'paused-batch',state:'paused',paid_work:true,likely_paid_items:1,provider_model:'gemini / retained-model',items:[],request:{operation:'transcribe',profile:null,options:{input_language:'mixed',output_language:{mode:'preserve'}}}}];
    if(path.endsWith('/start'))starts++;return route.fulfill({json:{api_version:'1',data}});
  });
  await page.goto('/src/features/home/tests/home.html');
  for(let i=0;i<2;i++){
    await page.getByRole('button',{name:'Reprocess saved recordings',exact:true}).click();await expect(page.getByRole('button',{name:'Resume batch explicitly'})).toBeDisabled();expect(starts).toBe(0);if(i===0)await page.reload();
  }
});
