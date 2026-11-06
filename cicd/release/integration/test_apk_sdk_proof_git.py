"""Offline local Git SDK proof controls; no real Cargo, network or build."""
import hashlib,json,os,pathlib,tempfile,unittest
from unittest.mock import patch
from apk_contract import Refusal,SDK_REPOSITORY
from apk_source_build import cargo_identity,run,sdk_release_source

class SdkReleaseProof(unittest.TestCase):
 def setUp(self):
  self.temporary=tempfile.TemporaryDirectory();self.addCleanup(self.temporary.cleanup)
  self.root=pathlib.Path(self.temporary.name).resolve()
  home=self.root/'home';home.mkdir()
  self.environment=patch.dict(os.environ,HOME=str(home),XDG_CONFIG_HOME=str(home/'.config'),
   GIT_AUTHOR_NAME='Fixture',GIT_AUTHOR_EMAIL='fixture@example.invalid',
   GIT_COMMITTER_NAME='Fixture',GIT_COMMITTER_EMAIL='fixture@example.invalid')
  self.environment.start();self.addCleanup(self.environment.stop)
  self.remote=self.root/'remote';self.remote.mkdir();self.git(self.remote,'init','--quiet')
  (self.remote/'.gitattributes').write_bytes(b'* -text\n')
  (self.remote/'Cargo.toml').write_bytes(b'[package]\nname="remarkable-open-sdk"\n')
  self.git(self.remote,'add','.');self.git(self.remote,'commit','--quiet','-m','fixture')
  self.commit=self.git(self.remote,'rev-parse','HEAD')
  self.tag='v1.2.3';self.git(self.remote,'tag','-a',self.tag,'-m','owned fixture tag')
  self.proof=self.root/'proof';self.cache=self.root/'cargo-generated'
  self.git(self.root,'clone','--quiet','--no-hardlinks',str(self.remote),str(self.proof))
  self.git(self.proof,'remote','set-url','origin',SDK_REPOSITORY)
  self.git(self.root,'clone','--quiet','--no-hardlinks','--no-tags',str(self.remote),str(self.cache))
  (self.cache/'.cargo-ok').touch()
  self.buddy=self.root/'buddy';self.buddy.mkdir()
  (self.buddy/'Cargo.toml').write_bytes(b'[package]\nname="fixture-buddy"\n')
  (self.buddy/'Cargo.lock').write_bytes(b'version=4\n')
  sdk_id='git+'+SDK_REPOSITORY+'?tag='+self.tag+'#remarkable-open-sdk@0.0.0'
  root_id='path+'+self.buddy.as_uri()+'#fixture-buddy@0.0.0'
  self.data=dict(packages=[dict(id=root_id,name='fixture-buddy',version='0.0.0',source=None,manifest_path=str(self.buddy/'Cargo.toml')),
   dict(id=sdk_id,name='remarkable-open-sdk',version='0.0.0',source='git+'+SDK_REPOSITORY+'?tag='+self.tag+'#'+self.commit,manifest_path=str(self.cache/'Cargo.toml'))],
   resolve=dict(root=root_id,nodes=[dict(id=root_id,features=[],dependencies=[sdk_id]),dict(id=sdk_id,features=[],dependencies=[])]))
  self.observations=0;self.fault=None;self.calls=[]
 def git(self,root,*args):return run(['git',*args],root)
 def transport(self,argv,cwd,*args,**kwargs):
  self.calls.append(argv)
  if argv[0]=='cargo':return json.dumps(self.data)
  if argv[:3]==['git','ls-remote','--exit-code']:
   self.assertEqual(argv[4],SDK_REPOSITORY)
   self.observations+=1
   if self.observations==2 and self.fault=='moved-remote':
    self.git(self.remote,'tag','-f','-a',self.tag,'-m','replacement annotation same commit')
   value=run([*argv[:4],str(self.remote),*argv[5:]],cwd,*args,**kwargs)
   if self.fault=='malformed':return value+'\nnot-a-ref'
   if self.fault=='duplicate':return value+'\n'+value.splitlines()[0]
   if self.fault=='wrong-commit':return value.replace(self.commit,'0'*40)
   if self.observations==1 and self.fault=='replace-manifest':
    path=self.proof/'Cargo.toml';data=path.read_bytes();path.unlink();path.write_bytes(data)
   if self.observations==1 and self.fault=='replace-proof':
    self.proof.rename(self.root/'retired-proof')
    self.git(self.root,'clone','--quiet','--no-hardlinks',str(self.remote),str(self.proof))
    self.git(self.proof,'remote','set-url','origin',SDK_REPOSITORY)
   return value
  return run(argv,cwd,*args,**kwargs)
 def identity(self,tag=True,proof=True):
  with patch('apk_source_build.run',side_effect=self.transport):
   return cargo_identity(self.buddy,'armv7-unknown-linux-gnueabihf',self.tag if tag else None,
                         sdk_tag_source=self.proof if proof else None)
 def test_actual_git_positive_tagless_cargo_and_untagged_mode(self):
  config=(self.cache/'.git/config').read_bytes();refs=self.git(self.cache,'for-each-ref')
  tagged=self.identity();self.assertEqual(tagged['sdk']['ref_kind'],'tagged-git')
  proof=tagged['sdk_release_source'];self.assertEqual(proof['snapshot']['source']['sha'],self.commit)
  self.assertEqual(proof['snapshot']['tag_oid'],self.git(self.proof,'rev-parse','refs/tags/'+self.tag))
  self.assertFalse(self.git(self.cache,'for-each-ref','refs/tags'))
  self.assertEqual((self.cache/'.git/config').read_bytes(),config);self.assertEqual(self.git(self.cache,'for-each-ref'),refs)
  self.assertFalse(any('fetch' in argv for argv in self.calls))
  self.observations=0;untagged=self.identity(tag=False,proof=False)
  self.assertEqual(untagged['sdk']['ref_kind'],'commit-pinned');self.assertIsNone(untagged['sdk']['tag'])
  self.assertNotIn('sdk_release_source',untagged);self.assertEqual(self.observations,0)
 def test_missing_pair_and_malformed_tag_refuse_before_cargo(self):
  for tag,proof,message in [(True,False,'requires --sdk-tag-source'),(False,True,'requires --sdk-tag')]:
   with self.subTest(tag=tag,proof=proof),self.assertRaisesRegex(Refusal,message):self.identity(tag,proof)
  self.assertFalse(self.calls)
  self.tag='v01.2.3'
  with self.assertRaisesRegex(Refusal,'exact stable tag'):self.identity()
 def test_missing_dirty_wrong_origin_and_wrong_head_proof(self):
  self.git(self.proof,'tag','-d',self.tag)
  with self.assertRaisesRegex(Refusal,'SDK release source proof refused'):self.identity()
  self.git(self.proof,'tag',self.tag,self.commit)
  (self.proof/'untracked').write_bytes(b'dirty')
  with self.assertRaisesRegex(Refusal,'clean exact source'):self.identity()
  (self.proof/'untracked').unlink();self.git(self.proof,'remote','set-url','origin','https://example.invalid/sdk')
  with self.assertRaisesRegex(Refusal,'canonical source'):self.identity()
  self.git(self.proof,'remote','set-url','origin',SDK_REPOSITORY)
  self.git(self.proof,'commit','--quiet','--allow-empty','-m','wrong HEAD')
  with self.assertRaisesRegex(Refusal,'tag/source mismatch'):self.identity()
 def test_mismatched_manifest_and_commit_refuse(self):
  with patch('apk_source_build.run',side_effect=self.transport):
   with self.assertRaisesRegex(Refusal,'Cargo manifest mismatch'):
    sdk_release_source(self.proof,self.tag,self.commit,'0'*64)
   with self.assertRaisesRegex(Refusal,'Cargo commit mismatch'):
    sdk_release_source(self.proof,self.tag,'0'*40,hashlib.sha256((self.cache/'Cargo.toml').read_bytes()).hexdigest())
 def test_malformed_duplicate_or_mismatched_remote_proof(self):
  for fault,message in [('malformed','malformed SDK remote'),('duplicate','malformed SDK remote'),('wrong-commit','remote tag/commit mismatch')]:
   self.fault=fault;self.observations=0
   with self.subTest(fault=fault),self.assertRaisesRegex(Refusal,message):self.identity()
 def test_remote_tag_moved_to_same_commit_is_refused(self):
  self.fault='moved-remote'
  with self.assertRaisesRegex(Refusal,'remote tag changed'):self.identity()
 def test_replaced_proof_manifest_is_refused(self):
  self.fault='replace-manifest'
  with self.assertRaisesRegex(Refusal,'proof changed during observation'):self.identity()
 def test_replaced_entire_proof_is_refused(self):
  self.fault='replace-proof'
  with self.assertRaisesRegex(Refusal,'proof changed during observation'):self.identity()
 def test_same_commit_tag_replacement_changes_bound_graph_identity(self):
  before=self.identity()
  self.git(self.remote,'tag','-f','-a',self.tag,'-m','new annotation')
  self.git(self.proof,'tag','-d',self.tag)
  # This is an owned fixture tag, never a Cargo/user ref. Import its exact new
  # object to model two individually coherent but different proof observations.
  self.git(self.proof,'fetch','--quiet',str(self.remote),'refs/tags/'+self.tag+':refs/tags/'+self.tag)
  after=self.identity()
  self.assertNotEqual(before['sdk_release_source'],after['sdk_release_source'])
  self.assertNotEqual(before['metadata_identity_sha256'],after['metadata_identity_sha256'])
 def test_local_tag_object_mismatch_and_independence(self):
  self.git(self.proof,'tag','-f','-a',self.tag,'-m','different annotation')
  with self.assertRaisesRegex(Refusal,'remote/local tag object mismatch'):self.identity()
  self.proof=self.cache
  with self.assertRaisesRegex(Refusal,'independent of Cargo checkout'):self.identity()

if __name__=='__main__':unittest.main()
