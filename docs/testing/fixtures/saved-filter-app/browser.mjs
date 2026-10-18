import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from '/opt/crewroom/playwright-core/index.mjs';
import assert from 'node:assert/strict';
const server=createServer(async(req,res)=>{
  const path=req.url==='/'?'index.html':req.url.slice(1);
  if(!['index.html','filters.mjs'].includes(path)){res.writeHead(404);res.end();return;}
  res.setHeader('Content-Type',path.endsWith('.mjs')?'text/javascript':'text/html');res.end(await readFile(path));
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({executablePath:'/usr/bin/chromium',args:['--no-sandbox','--disable-dev-shm-usage']});
try {
  const page=await browser.newPage();await page.goto(`http://127.0.0.1:${server.address().port}`);
  assert.equal(await page.locator('#empty').isVisible(),true);
  await page.getByRole('button',{name:'Export filters'}).click();assert.equal(await page.locator('#output').textContent(),'[]');
  await page.getByLabel('Filters JSON',{exact:true}).fill('[{"name":"Assigned","query":"assignee = me"}]');
  await page.getByRole('button',{name:'Export filters'}).click();assert.deepEqual(JSON.parse(await page.locator('#output').textContent()),[{name:'Assigned',query:'assignee = me'}]);
  assert.equal(await page.locator('#empty').isVisible(),false);
  for(const bad of ['broken','[{"name":"","query":"x"}]']) {
    await page.getByLabel('Filters JSON',{exact:true}).fill(bad);await page.getByRole('button',{name:'Export filters'}).click();
    assert.match(await page.getByRole('alert').textContent(),/name and query/);assert.equal(await page.locator('#output').textContent(),'');
  }
  const guide=await readFile('GUIDE.md','utf8');assert.match(guide,/No saved filters/);assert.match(guide,/Invalid JSON/);
  console.log('PASS browser: empty state, valid export, invalid input, cleared stale output, matching user guide');
} finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
