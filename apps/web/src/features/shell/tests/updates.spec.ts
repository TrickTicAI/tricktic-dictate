import {test,expect} from '@playwright/test';
for (const platform of ['Win32', 'MacIntel']) test(`${platform} update UI retains installed version, retries failures and reports busy installation`,async({page})=>{
  await page.route('http://tauri.localhost/**',async route=>{
    const response=await route.fetch({url:route.request().url().replace('http://tauri.localhost','http://127.0.0.1:15189')});
    await route.fulfill({response});
  });
  await page.addInitScript((platform)=>{
    Object.defineProperty(navigator, 'platform', { value: platform });
    const v={installed:'0.4.0',available:null as string|null,notes:null as string|null,stage:'unchecked',downloaded:0,total:null,checked_at:null as number|null,error:null as string|null};
    let downloads=0;
    (window as any).__TAURI__={core:{invoke:async(command:string)=>{
      if(command==='updates_check'){v.available='0.6.0';v.notes='A newer release';v.stage='available';v.checked_at=1700000000;}
      if(command==='updates_download'){
        downloads++;
        if(downloads===1){v.error='The update could not be downloaded or verified.';v.stage='available';}
        else {v.error=null;v.stage='ready';}
      }
      if(command==='updates_install')throw 'Finish recording and transcription before updating.';
      return structuredClone(v);
    }}};
  }, platform);
  await page.goto('http://tauri.localhost/src/features/shell/tests/updates-harness.html');
  await expect(page.getByText('0.4.0',{exact:true})).toBeVisible();
  await page.getByRole('button',{name:'Check for updates'}).click();
  await page.getByRole('button',{name:'Download 0.6.0'}).click();
  await expect(page.getByRole('alert')).toContainText('verified');
  await page.getByRole('button',{name:'Download 0.6.0'}).click();
  await page.getByRole('button',{name:'Update and restart'}).click();
  await expect(page.getByRole('alert')).toContainText('Finish recording');
  await expect(page.getByText('0.4.0',{exact:true})).toBeVisible();
  await expect(page.getByRole('button',{name:'Update and restart'})).toBeEnabled();
});

test('canceling a stalled download keeps Writer usable and allows a fresh download', async ({page}) => {
  await page.route('http://tauri.localhost/**', async route => {
    const response = await route.fetch({url:route.request().url().replace('http://tauri.localhost','http://127.0.0.1:15189')});
    await route.fulfill({response});
  });
  await page.addInitScript(() => {
    const v={installed:'0.20.0',available:'0.21.0',notes:null,stage:'available',downloaded:0,total:42,checked_at:null,error:null as string|null};
    let finish: (()=>void)|undefined;
    (window as any).__TAURI__={core:{invoke:async(command:string)=>{
      if(command==='updates_download') {
        v.stage='downloading';
        await new Promise<void>(resolve=>{finish=resolve;});
      }
      if(command==='updates_cancel') {v.stage='available';v.error='Update download canceled.';finish?.();}
      return structuredClone(v);
    }}};
  });
  await page.goto('http://tauri.localhost/src/features/shell/tests/updates-harness.html');
  await page.getByRole('button',{name:'Download 0.21.0'}).click();
  await expect(page.getByRole('button',{name:'Cancel download'})).toBeEnabled();
  await page.getByRole('button',{name:'Cancel download'}).click();
  await expect(page.getByRole('alert')).toContainText('canceled');
  await expect(page.getByRole('button',{name:'Download 0.21.0'})).toBeEnabled();
  await expect(page.getByText('0.20.0',{exact:true})).toBeVisible();
});

test('a missing Mac release reports a check failure without blaming the connection', async ({page}) => {
  await page.route('http://tauri.localhost/**', async route => {
    const response = await route.fetch({url:route.request().url().replace('http://tauri.localhost','http://127.0.0.1:15189')});
    await route.fulfill({response});
  });
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'platform', {value:'MacIntel'});
    const v={installed:'0.19.0',available:null,notes:null,stage:'unchecked',downloaded:0,total:null,checked_at:null,error:null as string|null};
    (window as any).__TAURI__={core:{invoke:async(command:string)=>{
      if(command==='updates_check') {
        v.stage='error';
        v.error='This update feed has no release for macOS (Apple Silicon). Your installed TrickTic Dictate is unchanged. Try again after a compatible release is published.';
      }
      return structuredClone(v);
    }}};
  });
  await page.goto('http://tauri.localhost/src/features/shell/tests/updates-harness.html');
  await page.getByRole('button',{name:'Check for updates'}).click();
  await expect(page.getByRole('status')).toHaveText('Could not check for updates');
  await expect(page.getByRole('alert')).toContainText('no release for macOS');
  await expect(page.getByRole('alert')).not.toContainText('connection');
  await expect(page.getByText('no successful check yet',{exact:false})).toBeVisible();
  await expect(page.getByRole('button',{name:'Check for updates'})).toBeEnabled();
  await expect(page.getByRole('button',{name:/Download/})).toHaveCount(0);
});
