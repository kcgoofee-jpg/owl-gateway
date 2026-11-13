import importlib.util
import os
from pathlib import Path
import tempfile
import time
import unittest

path=Path(__file__).resolve().parents[1]/'trim-build-cache.py'
spec=importlib.util.spec_from_file_location('trim_build_cache',path)
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class TrimTest(unittest.TestCase):
    def test_preserves_latest_cache_current_binary_and_symlink_target(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);target=root/'target';inc=target/'debug/incremental';deps=target/'debug/deps';inc.mkdir(parents=True);deps.mkdir()
            (target/'.rustc_info.json').write_text('{}')
            for n in range(3):
                p=inc/f'crate-a1234567{n}';p.mkdir();(p/'data').write_bytes(b'x'*20);os.utime(p,(100+n,100+n))
            private=root/'source';private.mkdir();(private/'important').write_text('keep');(inc/'source-a12345678').symlink_to(private,target_is_directory=True)
            old=deps/'test_a-0123456789abcdef';old.write_text('generated');old.chmod(0o755);old.with_suffix('.d').touch();os.utime(old,(100,100))
            current=deps/'cli_a-fedcba9876543210';current.write_text('current');current.chmod(0o755);current.with_suffix('.d').touch();os.utime(current,(100,100));os.link(current,target/'debug/nai-bots')
            paths,stats=module.plan(target,25,2,now=time.time())
            self.assertIn(old.resolve(),paths);self.assertNotIn(current.resolve(),paths);self.assertEqual(stats['incremental_after_bytes'],20)
            module.apply(target,paths)
            self.assertEqual((private/'important').read_text(),'keep');self.assertTrue((target/'debug/nai-bots').exists());self.assertTrue((inc/'crate-a12345672').exists())
    def test_requires_cargo_marker_and_refuses_external_paths(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            with self.assertRaises(ValueError):module.plan(root,1,2)
            (root/'.rustc_info.json').touch();outside=root/'important';outside.write_text('keep')
            with self.assertRaises(ValueError):module.apply(root,[outside])
            self.assertTrue(outside.exists())

if __name__=='__main__':unittest.main()
