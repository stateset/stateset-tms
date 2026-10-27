#!/usr/bin/env python3
"""Profile CPU decode phases on a retained, qualified service fixture."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
from runtime_contracts import ROOT, fingerprint, require

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixture',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    if args.output.exists():parser.error('preserve the original report; choose another output path')
    source=fingerprint(ROOT);fixture=hashlib.sha256(args.fixture.read_bytes()).hexdigest()
    harness={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in ['scripts/profile-read-decode.py','examples/read_decode_profile.rs','src/read_json.rs']}
    command=['cargo','run','--release','--offline','--locked','--manifest-path',str(ROOT/'Cargo.toml'),'--example','read_decode_profile','--',str(args.fixture.resolve())]
    result=subprocess.run(command,cwd=ROOT,text=True,stdout=subprocess.PIPE,check=True,env=dict(os.environ,RUSTC_WRAPPER='',CARGO_INCREMENTAL='0',CARGO_BUILD_JOBS='2',CARGO_TARGET_DIR=str(ROOT/'target')))
    report=json.loads(result.stdout)
    require(report['optimized'],'release profile required')
    require(fixture==report['fixture_sha256']==hashlib.sha256(args.fixture.read_bytes()).hexdigest(),'fixture changed')
    require(source==fingerprint(ROOT),'application source changed during profiling')
    require(all(value==hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name,value in harness.items()),'profile source changed')
    report['provenance']={'source':source,'harness':harness,'command':command,'binary_sha256':hashlib.sha256((ROOT/'target/release/examples/read_decode_profile').read_bytes()).hexdigest(),'rustc':subprocess.check_output(['rustc','--version'],text=True).strip()}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    with args.output.open('x') as stream:stream.write(json.dumps(report,indent=2)+'\n')
    print('REPORT',args.output)

if __name__=='__main__':main()
