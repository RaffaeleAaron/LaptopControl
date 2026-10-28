#!/usr/bin/env python3
"""Check public package names/versions in Cargo.lock against OSV; never upload code."""
import json
from pathlib import Path
import sys
import tomllib
from urllib.request import Request, urlopen

root=Path(__file__).resolve().parents[1]
packages=[p for p in tomllib.loads((root/'Cargo.lock').read_text())['package'] if p.get('source','').startswith('registry+')]
findings=[]
try:
    for start in range(0,len(packages),100):
        batch=packages[start:start+100]
        query={'queries':[{'package':{'name':p['name'],'ecosystem':'crates.io'},'version':p['version']} for p in batch]}
        request=Request('https://api.osv.dev/v1/querybatch',data=json.dumps(query).encode(),headers={'Content-Type':'application/json'})
        with urlopen(request,timeout=30) as response:results=json.load(response)['results']
        if len(results)!=len(batch):raise ValueError('Incomplete advisory response')
        for package,result in zip(batch,results):
            for advisory in result.get('vulns',[]):findings.append((package['name'],package['version'],advisory['id']))
except Exception as error:
    print('Advisory check did not complete:',error,file=sys.stderr)
    raise SystemExit(2)
for name,version,advisory in findings:print(name,version,advisory,'https://osv.dev/vulnerability/'+advisory)
print(f'{len(packages)} locked Rust packages checked; {len(findings)} published advisory matches')
raise SystemExit(bool(findings))
