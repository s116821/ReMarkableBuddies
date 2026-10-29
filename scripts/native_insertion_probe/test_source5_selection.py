"""Closed source5 selectors and preservation; pending pins never grant authority."""
import unittest
from unittest.mock import patch
from open_document_timing import (SOURCE5,SOURCE5_CONFIG_LINE,V2_CONFIG_LINE,
    PRESERVED_PAGES,fixture_inventory_selection,fixture_source_selection,
    validate_v2_binding,validate_v2_config,SOURCE5_SDK_REVISION,SOURCE5_SDK_SOURCES)
from test_preserved_inventory import selected,inventory
from prepare_packet import partition
from coordinator import packet_files

def source5():
    return selected()|{'fixture_inventory_variant':SOURCE5,'sdk_revision':SOURCE5_SDK_REVISION,
        'source_witness_variant':'source5-footer-then-body-v1',
        'sdk_source_hashes':SOURCE5_SDK_SOURCES,'source_index':5,
        'source_page_id':PRESERVED_PAGES[5],'neighbor_index':6,
        'neighbor_page_id':PRESERVED_PAGES[6],'insertion_index':6}

class Source5Selection(unittest.TestCase):
    def test_witness_selector_closed_and_variant_specific(self):
        from open_document_timing import source_witness_selection
        s=source5()
        for value in ('source5-footer-then-body-v1','source5-stable-body-idle-v1'):
            self.assertEqual(source_witness_selection(s|{'source_witness_variant':value}),value)
        for value in (None,True,'unknown'):
            with self.assertRaises(ValueError):partition(s|{'source_witness_variant':value},inventory(s))
        missing=s.copy();del missing['source_witness_variant']
        with self.assertRaises(ValueError):partition(missing,inventory(s))
        self.assertIsNone(source_witness_selection(selected()))
        with self.assertRaises(ValueError):partition(selected()|{'source_witness_variant':'source5-footer-then-body-v1'},inventory(selected()))
    def test_explicit_tuple_no_defaults_or_type_aliases(self):
        s=source5()
        self.assertEqual(fixture_inventory_selection(s,'before_page_ids'),(SOURCE5,8))
        self.assertEqual(fixture_source_selection(s,'before_page_ids'),(5,6))
        self.assertEqual(fixture_source_selection(selected(),'before_page_ids'),(0,1))
        keys=['source_index','source_page_id','neighbor_index','neighbor_page_id','insertion_index']
        for key in keys:
            missing=s.copy();del missing[key]
            for bad in [missing,s|{key:None},s|{key:True},s|{key:'0'}]:
                with self.subTest(key=key,bad=bad),self.assertRaises(ValueError):
                    fixture_inventory_selection(bad,'before_page_ids')
        for key,value in [('source_index',0),('neighbor_index',1),('insertion_index',1),
                          ('source_page_id',PRESERVED_PAGES[0]),('neighbor_page_id',PRESERVED_PAGES[1])]:
            with self.assertRaises(ValueError):fixture_inventory_selection(s|{key:value},'before_page_ids')
    def test_exact_config_variant_without_comment_or_string_authority(self):
        good=V2_CONFIG_LINE+'\n'+SOURCE5_CONFIG_LINE
        validate_v2_config(good,SOURCE5)
        for text in [V2_CONFIG_LINE,good+'\n'+SOURCE5_CONFIG_LINE,
                     V2_CONFIG_LINE+'\n//'+SOURCE5_CONFIG_LINE,
                     V2_CONFIG_LINE+'\nconst char*x="'+SOURCE5_CONFIG_LINE+'";']:
            with self.assertRaises(ValueError):validate_v2_config(text,SOURCE5)
        for variant in ['legacy','preserved-eight-after-9005','unknown']:
            with self.assertRaises(ValueError):validate_v2_config(good,variant)
    def test_earlier_selector_cannot_silently_reinterpret_source5_tuple(self):
        from open_document_timing import ORIGINAL_PAGES
        fields={k:source5()[k] for k in ('source_index','source_page_id','neighbor_index','neighbor_page_id','insertion_index')}
        older=[selected(),selected()|{'fixture_inventory_variant':'legacy','before_page_ids':list(ORIGINAL_PAGES)}]
        for old in older:
            for key,value in fields.items():
                with self.subTest(variant=old['fixture_inventory_variant'],key=key),self.assertRaises(ValueError):
                    fixture_source_selection(old|{key:value},'before_page_ids')
            with self.assertRaises(ValueError):validate_v2_binding(old|fields)
    def test_preservation_and_packet_tool_are_variant_specific(self):
        s=source5();files=inventory(s)
        _,_,pages,runtime,immutable,mutable=partition(s,files)
        self.assertEqual((len(pages),len(runtime),len(immutable),len(mutable)),(8,10,16,2))
        self.assertIn('owned_relative_gesture',packet_files(s))
        self.assertNotIn('owned_relative_gesture',packet_files(selected()))
        for target in [PRESERVED_PAGES[1],PRESERVED_PAGES[6]]:
            with self.assertRaises(ValueError):partition(s|{'target_page_id':target},files)
    def test_pending_reviewed_sdk_pins_refuse(self):
        validate_v2_binding(source5())
        with patch('open_document_timing.SOURCE5_SDK_REVISION',None):
            with self.assertRaises(ValueError):validate_v2_binding(source5())
        for bad in [source5()|{'sdk_revision':'7e3cca6fa15491a32a01b71f7f209a1d0d5dfec9'},
                    source5()|{'sdk_source_hashes':{}}]:
            with self.assertRaises(ValueError):validate_v2_binding(bad)

if __name__=='__main__':unittest.main()
