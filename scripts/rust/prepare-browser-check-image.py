#!/usr/bin/env python3
"""Copy the accepted Chromium image layers, removing only its inherited VOLUME metadata."""
import argparse,copy,hashlib,io,json,subprocess,tarfile
from pathlib import Path
SOURCE='sha256:425dd73955b5b99e61a37954480a0de825d203b55f8fda4d4377f4e29fdb8ea7'
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',required=True,type=Path);a=p.parse_args();root=a.output.resolve();root.mkdir(parents=True,exist_ok=False,mode=0o700)
source=json.loads(subprocess.check_output(['docker','image','inspect',SOURCE],text=True))[0];assert source['Id']==SOURCE
original=root/'source.tar';changed=root/'no-volumes.tar'
subprocess.run(['docker','image','save','--output',str(original),SOURCE],check=True,timeout=180)
with tarfile.open(original) as archive:
 manifest=json.load(archive.extractfile('manifest.json'));assert len(manifest)==1
 entry=manifest[0];raw=archive.extractfile(entry['Config']).read();config=json.loads(raw);replacement=copy.deepcopy(config)
 removed=replacement['config'].pop('Volumes',None);assert removed=={'/var/lib/postgresql/data':{}}
 encoded=json.dumps(replacement,sort_keys=True,separators=(',',':')).encode();digest=hashlib.sha256(encoded).hexdigest();tag='crewroom-native-browser-check:'+digest
 # Write Docker's classic image archive so no stale OCI manifest/index can select
 # the original configuration. Layer members are byte-for-byte original blobs.
 replacement_manifest=[{**entry,'Config':digest+'.json','RepoTags':[tag]}]
 with tarfile.open(changed,'w') as output:
  def add(name,data):
   item=tarfile.TarInfo(name);item.size=len(data);item.mode=0o600;output.addfile(item,io.BytesIO(data))
  add(digest+'.json',encoded);add('manifest.json',json.dumps(replacement_manifest,separators=(',',':')).encode())
  for name in entry['Layers']:
   member=archive.getmember(name);output.addfile(member,archive.extractfile(member))
with (root/'load.log').open('w') as log:subprocess.run(['docker','image','load','--input',str(changed)],check=True,stdout=log,stderr=subprocess.STDOUT,timeout=180)
built=json.loads(subprocess.check_output(['docker','image','inspect',tag],text=True))[0]
assert not built['Config'].get('Volumes') and built['RootFS']==source['RootFS']
expected=copy.deepcopy(source['Config']);expected.pop('Volumes',None);assert built['Config']==expected
receipt={'sourceImage':SOURCE,'sourceRootFilesystem':source['RootFS'],'sourceConfigSha256':hashlib.sha256(raw).hexdigest(),'image':built['Id'],'tag':tag,'imageConfigSha256':digest,'rootFilesystem':built['RootFS'],'removedVolumes':removed,'rootFilesystemByteIdentical':True,'otherImageConfigIdentical':True,'preparationScriptSha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'boundary':'Only the accepted fixture image config.Volumes was removed. Every original filesystem layer is unchanged; no application/container ownership check is relaxed, and no live service image is changed.'}
(root/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');(root/'image-id').write_text(built['Id']+'\n');print(json.dumps(receipt,indent=2))
# These archives contain identical public fixture layers; the small receipt and
# loaded immutable image preserve reproduction inputs without retaining 1.3 GiB.
original.unlink();changed.unlink()
