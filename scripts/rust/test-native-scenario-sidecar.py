#!/usr/bin/env python3
"""Final signed package binding accepts exact bytes and rejects stale/tampered provenance."""
import hashlib,importlib.util,json,tempfile,unittest
from pathlib import Path
spec=importlib.util.spec_from_file_location('native_scenarios',Path(__file__).with_name('verify-native-scenarios.py').resolve());module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
class SignedReceiptTests(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name);self.bundle=self.root/'Fixture.app';self.bin=self.bundle/'Contents/MacOS';self.bin.mkdir(parents=True);self.internal=self.bundle/'Contents/Resources/build-receipt.json';self.internal.parent.mkdir();self.sha='8'*40
  self.internal.write_text(json.dumps({'sourceRevision':self.sha,'binaries':{name:{'preBundleSignatureSha256':'0'*64} for name in ['crewroomd','crewroom-process','crewroom-host']}}))
  self.binaries={}
  for name in ['crewroomd','crewroom-process','crewroom-host']:
   path=self.bin/name;path.write_text('final signed '+name);self.binaries[name]={'path':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
  self.binaries['provider-daemon']={'path':'/separate/example','sha256':'1'*64}
  self.receipt={'sourceRevision':self.sha,'bundleSignatureVerified':True,'internalProvenanceSha256':hashlib.sha256(self.internal.read_bytes()).hexdigest(),'binaries':{name:{'packagedSha256':row['sha256']} for name,row in self.binaries.items() if name!='provider-daemon'}};self.sidecar=self.root/'Fixture.app.receipt.json';self.save()
 def tearDown(self):self.temp.cleanup()
 def save(self):self.sidecar.write_text(json.dumps(self.receipt))
 def check(self):return module.final_package_receipt(self.bin,self.binaries,self.sha)
 def test_final_hashes_pass_despite_different_presign_hashes(self):self.assertEqual(len(self.check()['verifiedPackagedBinaries']),3)
 def test_wrong_candidate_rejected(self):self.receipt['sourceRevision']='7'*40;self.save();self.assertRaisesRegex(ValueError,'source revision',self.check)
 def test_tampered_signed_binary_rejected(self):self.binaries['crewroom-host']['sha256']='2'*64;self.assertRaisesRegex(ValueError,'final signed sidecar',self.check)
 def test_internal_receipt_alone_never_suffices(self):self.sidecar.unlink();self.assertRaisesRegex(ValueError,'Missing regular package receipt',self.check)
 def test_explicit_candidate_required(self):self.assertRaisesRegex(ValueError,'exact full candidate SHA',module.final_package_receipt,self.bin,self.binaries,None)
 def test_internal_provenance_tamper_rejected(self):self.internal.write_text(self.internal.read_text()+'\n');self.assertRaisesRegex(ValueError,'provenance bytes',self.check)
 def test_unsigned_receipt_rejected(self):self.receipt['bundleSignatureVerified']=False;self.save();self.assertRaisesRegex(ValueError,'signature verification',self.check)
 def test_development_binaries_need_no_package(self):self.assertIsNone(module.final_package_receipt(self.root/'target/debug',{},None))
unittest.main()
