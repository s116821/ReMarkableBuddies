"""Exact fixture/config/partition and sealed no-transport drift controls."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from open_document_timing import (PRESERVED_EIGHT, PRESERVED_PAGES, ORIGINAL_PAGES,
    FIXTURE_DOCUMENT, EIGHT_CONFIG_LINE, EIGHT_SDK_REVISION, EIGHT_SDK_SOURCES,
    V2_CONFIG_LINE, V2_CONTRACT, PRODUCTION_V2, fixture_inventory_selection,
    validate_v2_binding, validate_v2_config, V2_PUBLISHER_SHA256)
from prepare_packet import partition
from open_document_transport import SSHAdapter
import coordinator
import test_host_profile as profile_tests

def selected():
    return {'experiment_kind':PRODUCTION_V2,'document_id':FIXTURE_DOCUMENT,
        'fixture_inventory_variant':PRESERVED_EIGHT,'before_page_ids':list(PRESERVED_PAGES),
        'target_page_id':'12345678-1234-4567-8901-123456789abc',
        'production_contract':V2_CONTRACT,'sdk_revision':EIGHT_SDK_REVISION,
        'sdk_source_hashes':EIGHT_SDK_SOURCES,'sdk_setup_variant':'AfterOpen95s',
        'sdk_configuration_sha256':'a'*64}

def inventory(s):
    prefix='/home/root/.local/share/remarkable/xochitl/'+s['document_id']
    paths={prefix+'.'+ext for ext in ('content','metadata','local','pagedata','pdf')}
    paths|={prefix+'/'+page+'.rm' for page in s['before_page_ids'][:5]}
    paths|={prefix+'.thumbnails/'+page+'.png' for page in s['before_page_ids']}
    paths|={'/usr/lib/libQt6'+name+'.so.6.10.3' for name in ('Core','Qml','Gui','Quick','Network')}
    paths|={'/usr/lib/libstdc++.so.6','/lib/libc.so.6','/lib/libm.so.6','/lib/libgcc_s.so.1','/lib/ld-linux-armhf.so.3'}
    return [{'path':path,'sha256':'a'*64} for path in sorted(paths)]

class PreservedInventoryTests(unittest.TestCase):
    def test_exact_eight_binding_refuses_order_count_source_drift(self):
        s=selected();self.assertEqual(fixture_inventory_selection(s,'before_page_ids'),(PRESERVED_EIGHT,8))
        validate_v2_binding(s)
        for field,value in [('sdk_revision','39e386dd2927f771ccf5849b01edc60cffada49b'),
            ('sdk_source_hashes',{}),('sdk_setup_variant','Legacy40s'),
            ('fixture_inventory_variant',None),('fixture_inventory_variant','unknown'),
            ('fixture_inventory_variant',8),('document_id','12345678-1234-4567-8901-123456789abc')]:
            with self.subTest(field=field),self.assertRaises(ValueError):validate_v2_binding(s|{field:value})
        for pages in [list(ORIGINAL_PAGES),list(reversed(PRESERVED_PAGES)),
            list(PRESERVED_PAGES)+[s['target_page_id']],list(PRESERVED_PAGES[:1])+list(PRESERVED_PAGES[2:]),
            list(PRESERVED_PAGES[:1])*8]:
            with self.subTest(pages=pages),self.assertRaises(ValueError):fixture_inventory_selection(s|{'before_page_ids':pages},'before_page_ids')

    def test_config_lexical_selector_and_disagreement(self):
        validate_v2_config(V2_CONFIG_LINE+'\n'+EIGHT_CONFIG_LINE,PRESERVED_EIGHT)
        validate_v2_config(V2_CONFIG_LINE)
        for text in [V2_CONFIG_LINE,V2_CONFIG_LINE+'\n// '+EIGHT_CONFIG_LINE,
            V2_CONFIG_LINE+'\nconst char *x="'+EIGHT_CONFIG_LINE+'";',
            V2_CONFIG_LINE+'\n'+EIGHT_CONFIG_LINE+'\n'+EIGHT_CONFIG_LINE,
            V2_CONFIG_LINE+'\n'+EIGHT_CONFIG_LINE.replace('PreservedEightAfter9005','Legacy')]:
            with self.subTest(text=text),self.assertRaises(ValueError):validate_v2_config(text,PRESERVED_EIGHT)
        with self.assertRaises(ValueError):validate_v2_config(V2_CONFIG_LINE+'\n'+EIGHT_CONFIG_LINE,'legacy')

    def test_exact_28_paths_protect_inserted_page_and_fit_output_bound(self):
        s=selected();files=inventory(s);self.assertEqual(len(files),28)
        _,_,pages,runtime,immutable,mutable=partition(s,files)
        self.assertEqual((len(pages),len(runtime),len(immutable),len(mutable)),(8,10,16,2))
        prefix='/home/root/.local/share/remarkable/xochitl/'+FIXTURE_DOCUMENT;preserved=PRESERVED_PAGES[1]
        self.assertIn(prefix+'/'+preserved+'.rm',{x['path'] for x in immutable})
        self.assertIn(prefix+'.thumbnails/'+preserved+'.png',{x['path'] for x in immutable})
        self.assertLessEqual(sum(len(x['sha256']+'  '+x['path']+'\n') for x in runtime+immutable),4096)
        for changed in [files[:-1],files+[{'path':'/extra','sha256':'a'*64}],
            [x|{'path':x['path'].replace(preserved,'00000000-0000-0000-0000-000000000001')} for x in files]]:
            with self.subTest(changed=changed),self.assertRaises(ValueError):partition(s,changed)
        for target in [preserved,FIXTURE_DOCUMENT,'00000000-0000-0000-0000-000000000000']:
            with self.assertRaises(ValueError):partition(s|{'target_page_id':target},files)

    def test_profile_receipt_selector_count_and_fresh_reference_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);receipt,profile=profile_tests.ProfileTests().fixture(root);s=selected()
            profile.update(s);profile.pop('before_page_ids');profile['page_ids']=s['before_page_ids']
            profile['host_sequence_contract']='bounded-host-v6-production-v2';profile['library_tap_variant']='primary-1641396'
            receipt.update(experiment_kind=PRODUCTION_V2,document_id=FIXTURE_DOCUMENT,
                before_page_ids=s['before_page_ids'],fixture_inventory_variant=PRESERVED_EIGHT,
                library_tap_variant='primary-1641396',target_page_id=s['target_page_id'])
            profile['target_id']=s['target_page_id']
            receipt['files']['publish_creation_arm']=profile['tools']['publish_creation_arm']=V2_PUBLISHER_SHA256
            entries={f'/fixture/immutable/{i}':coordinator.sha(str(i).encode()) for i in range(16)}
            profile['immutable_hashes']={k:v for k,v in profile['immutable_hashes'].items() if 'document-immutable.files' not in k}|entries
            (root/'document-immutable.files').write_text(''.join(f'{v}  {k}\n' for k,v in entries.items()))
            (root/'host-profile.json').write_text(json.dumps(profile))
            with self.assertRaises(ValueError):coordinator.load_host_profile(root,receipt)
            with patch('coordinator.EIGHT_LIBRARY_SHA256',profile['references']['library-reference.png']):
                self.assertEqual(coordinator.load_host_profile(root,receipt),profile)
                for value in [None,'legacy','unknown',8]:
                    with self.assertRaises(ValueError):coordinator.load_host_profile(root,receipt|{'fixture_inventory_variant':value})
                changed=copy.deepcopy(profile);changed['page_ids']=list(reversed(changed['page_ids']))
                (root/'host-profile.json').write_text(json.dumps(changed))
                with self.assertRaises(ValueError):coordinator.load_host_profile(root,receipt)

    def test_transport_inventory_latch_prevents_later_consistent_switch(self):
        s=selected();profile=s|{'page_ids':s['before_page_ids'],'nonce':'1'*32}
        receipt=s|{'remote_root':'/run/rmb-qt-probe-'+ '1'*32};calls=[]
        with tempfile.TemporaryDirectory() as temporary:
            adapter=SSHAdapter([],receipt,profile,temporary,lambda *args:calls.append(args) or {'timeout':False,'exit':0,'stdout':''},b'')
            adapter.call('true');self.assertEqual(len(calls),1)
            profile['fixture_inventory_variant']=receipt['fixture_inventory_variant']='legacy'
            with self.assertRaises(ValueError):adapter.call('true')
            self.assertEqual(len(calls),1)
            profile['fixture_inventory_variant']=PRESERVED_EIGHT
            with self.assertRaises(ValueError):SSHAdapter([],receipt,profile,temporary,lambda *args:calls.append(args),b'')
            self.assertEqual(len(calls),1)

if __name__=='__main__':unittest.main()
