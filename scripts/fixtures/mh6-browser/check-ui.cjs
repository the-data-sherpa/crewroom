const assert=require('node:assert/strict');
const {chromium}=require('/opt/playwright-core');
(async()=>{
 const browser=await chromium.launch({executablePath:'/usr/bin/chromium',headless:true,args:['--no-sandbox','--disable-dev-shm-usage']});
 try {
  const page=await browser.newPage();
  for(const width of [1280,390]){
   await page.setViewportSize({width,height:844});await page.goto('file://'+process.cwd()+'/index.html');
   const search=page.getByRole('searchbox',{name:/search people/i});await search.fill('  GRACE  ');
   assert.deepEqual(await page.locator('#people li:visible').allTextContents(),['Grace Hopper']);
   await search.fill('no such person');assert.equal(await page.locator('#people li:visible').count(),0);await page.getByText('No people match',{exact:true}).waitFor({state:'visible'});
   const live=await page.locator('#empty').getAttribute('aria-live');assert.ok(live==='polite'||live==='assertive'||await page.locator('#empty').getAttribute('role')==='status','Empty state must be announced');
   await search.press('Escape');assert.equal(await search.inputValue(),'');assert.equal(await page.locator('#people li:visible').count(),3);assert.equal(await search.evaluate(e=>e===document.activeElement),true);
   assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No horizontal overflow');
   const path=(process.env.SCREENSHOTS??'/tmp')+'/ui-'+width+'.png';await page.screenshot({path,fullPage:true});
   console.log('PASS browser: filtering, whitespace/case, empty announcement, Escape, focus, layout width '+width);
  }
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
