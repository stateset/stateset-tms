#!/usr/bin/env python3
"""Owned loopback HTTP/PostgreSQL workloads; records observations, not a capacity SLA."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import shutil
import signal
import sys
import threading
import time
import urllib.error
import urllib.request

from runtime_contracts import ROOT, Runtime, fingerprint, literal, require

LARGE = "freight-capacity-large-synthetic-credential-only"
PROBE = "freight-capacity-probe-synthetic-credential-only"


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


class Capacity(Runtime):
    def __init__(self, args):
        super().__init__(args)
        self.baseline = json.loads(args.baseline.read_text()) if args.baseline else None
        self.source = fingerprint(ROOT)
        self.harness = {p: digest((ROOT / p).read_bytes()) for p in ("scripts/qualify-service-capacity.py", "scripts/runtime_contracts.py", "examples/service_capacity_fixture.rs")}
        self.clock = self.baseline["fixture_clock"] if self.baseline else dt.datetime.now(dt.timezone.utc).isoformat()
        self.report.update(scope="synthetic actual Freight HTTP/PostgreSQL observations", schema=1, fixture_clock=self.clock, settings={"sizes":args.sizes,"requests":args.requests,"concurrency":[1,4,8],"api_async_threads":2,"api_cpu_quota":"not imposed","postgres_cpu_quota":2,"postgres_memory_mib":768,"idle_seconds":35,"periodic_workers":"enabled","identity":"owned static scoped credentials; Clerk/TLS/browser excluded"}, workloads=[], harness=self.harness, host={"platform":sys.platform,"logical_cpus":os.cpu_count()})
        self.secrets += [LARGE, PROBE]
        if self.baseline:
            require(self.baseline["settings"]==self.report["settings"], "Workload settings changed")
            require(self.baseline["harness"]["examples/service_capacity_fixture.rs"]==self.harness["examples/service_capacity_fixture.rs"], "Fixture generator changed")

    def build(self):
        env = dict(os.environ, RUSTC_WRAPPER="", CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="2", CARGO_TARGET_DIR=str(ROOT/"target"))
        with (self.directory/"build.log").open("w") as log:
            subprocess.run(["cargo","build","--offline","--locked","--release","--manifest-path",str(ROOT/"Cargo.toml"),"--bin","stateset-freight","--example","service_capacity_fixture"],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
        require(self.source==fingerprint(ROOT), "Application changed during build")
        self.binaries={"freight":ROOT/"target/release/stateset-freight"}
        self.generator=ROOT/"target/release/examples/service_capacity_fixture"
        self.report["builds"]={"freight":{"source":self.source,"binary_sha256":digest(self.binaries["freight"].read_bytes())},"fixture":{"binary_sha256":digest(self.generator.read_bytes())}}

    def start_api(self):
        self.spawn("freight", {"DATABASE_URL":f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime","FREIGHT_DEMO":"false","TOKIO_WORKER_THREADS":"2","FREIGHT_BIND_ADDR":f"127.0.0.1:{self.ports['freight']}","FREIGHT_INTEGRATIONS_JSON":"{}","FREIGHT_API_KEYS_JSON":json.dumps({LARGE:{"tenant_id":"large","subject":"user:capacity","role":"operations"},PROBE:{"tenant_id":"probe","subject":"user:probe","role":"viewer"}})})
        self.wait(lambda:self.http("freight","/health",token=LARGE),"Freight ready",60)

    def snapshot(self):
        return self.rows("freight_runtime", "SELECT tenant_id,xmin::text AS revision,updated_at,md5(data::text) AS data_md5,pg_column_size(data) AS stored_bytes,octet_length(data::text) AS json_bytes FROM freight_workspaces ORDER BY tenant_id")

    def physical_snapshot(self):
        return self.rows('freight_runtime', "SELECT md5((data->'carrier_submissions')::text) AS native_md5,md5((data->'stop_events')::text) AS gates_md5,md5((data->'tenders')::text) AS tenders_md5 FROM freight_workspaces WHERE tenant_id='large'")

    def request(self, spec, queued=None):
        kind,path,token,body,key = spec
        start=time.perf_counter()
        headers={"Authorization":"Bearer "+token}
        if key:headers["Idempotency-Key"]=key
        if body is not None:headers["Content-Type"]="application/json"
        req=urllib.request.Request(self.urls['freight']+path,data=None if body is None else json.dumps(body).encode(),headers=headers,method="GET" if body is None else "POST")
        try:
            try:response=self.opener.open(req,timeout=60)
            except urllib.error.HTTPError as error:response=error
            with response:
                raw=response.read();status=response.status
            error=None
        except (OSError,TimeoutError) as failure:
            raw=b"";status=0;error=type(failure).__name__
        end=time.perf_counter()
        return {"kind":kind,"status":status,"response_ms":(end-start)*1000,"client_queue_ms":(start-(queued or start))*1000,"total_ms":(end-(queued or start))*1000,"bytes":len(raw),"error":error},raw

    def monitor(self, stop, observations):
        while not stop.is_set():
            started=time.perf_counter()
            try:
                rows=self.rows("freight_runtime", "SELECT state,wait_event_type,wait_event,count(*) AS connections,sum(cardinality(pg_blocking_pids(pid))) AS blockers FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() GROUP BY state,wait_event_type,wait_event")
                stat=Path(f"/proc/{self.processes['freight'].pid}/stat").read_text().rsplit(')',1)[1].split()
                observations.append({"at":time.monotonic(),"postgres":rows,"api_cpu_seconds":(int(stat[11])+int(stat[12]))/os.sysconf('SC_CLK_TCK'),"api_rss_bytes":int(stat[21])*os.sysconf('SC_PAGE_SIZE'),"sample_ms":(time.perf_counter()-started)*1000})
            except Exception as error:
                observations.append({"error":type(error).__name__})
            stop.wait(0.2)

    def wave(self, name, specs, concurrency):
        samples=[];stop=threading.Event();observations=[]
        monitor=threading.Thread(target=self.monitor,args=(stop,observations),daemon=True)
        monitor.start();started=time.perf_counter()
        try:
            with ThreadPoolExecutor(max_workers=concurrency) as pool:
                futures=[pool.submit(self.request,spec,time.perf_counter()) for spec in specs]
                results=[f.result() for f in futures]
        finally:
            stop.set();monitor.join(10)
        elapsed=time.perf_counter()-started
        require(not monitor.is_alive(), "PostgreSQL monitor did not stop")
        for sample,raw in results:
            samples.append(sample)
        groups={}
        for kind in sorted({s['kind'] for s in samples}):
            values=sorted(s['response_ms'] for s in samples if s['kind']==kind)
            groups[kind]={"samples":len(values),"p50_ms":values[math.ceil(len(values)*.5)-1],"p95_ms":values[math.ceil(len(values)*.95)-1],"p99_ms":values[math.ceil(len(values)*.99)-1],"max_ms":max(values)}
        report={"name":name,"concurrency":concurrency,"requests":len(specs),"elapsed_seconds":elapsed,"observed_requests_per_second":len(specs)/elapsed,"latency":groups,"samples":samples,"resources":observations,"monitoring_valid":bool(observations) and all('error' not in r for r in observations)}
        print(f"WAVE {name}: {len(specs)} requests, {elapsed:.2f}s, "+', '.join(f"{k} p95={v['p95_ms']:.1f}ms" for k,v in groups.items()),flush=True)
        return report,results

    def checks(self):
        self.start_api();self.stop('freight')
        version=int(self.sql('freight_runtime', 'SELECT version FROM freight_storage_format'))
        for size in self.args.sizes:
            self.sql('freight_runtime', 'TRUNCATE freight_load_search,freight_load_catalog,freight_workspaces,freight_document_content CASCADE')
            fixtures={}
            for tenant,count,prefix in [('large',size,f'capacity-{size}'),('probe',5,f'probe-{size}')]:
                raw=subprocess.check_output([str(self.generator),str(count),prefix,self.clock],cwd=self.directory)
                data=json.loads(raw);fixtures[tenant]={"sha256":digest(raw),"counts":data['counts']}
                if tenant=='large':originals={key:data['workspace'][key] for key in ['carrier_submissions','stop_events','tenders']}
                (self.directory/f'fixture-{size}-{tenant}.json').write_bytes(raw)
                workspace=data['workspace'];workspace['_freight_format']=version
                self.command(['psql',f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime",'-X','-v','ON_ERROR_STOP=1'],label='owned fixture insert',input='INSERT INTO freight_workspaces(tenant_id,data) VALUES('+literal(tenant)+','+literal(json.dumps(workspace))+ '::jsonb);',env={'PATH':os.defpath,'LC_ALL':'C'})
            if self.baseline:
                prior=next(w for w in self.baseline['workloads'] if w['shipments']==size)
                require(fixtures==prior['fixtures'], 'Before/after fixture records differ')
            before=self.snapshot();physical_before=self.physical_snapshot();self.start_api()
            time.sleep(35)
            after=self.snapshot()
            require([r['data_md5'] for r in before]==[r['data_md5'] for r in after], 'Idle workers changed business records')
            idle={"before":before,"after":after,"unchanged_revisions":all(a['revision']==b['revision'] for a,b in zip(before,after))}
            workload={"shipments":size,"fixtures":fixtures,"idle":idle,"waves":[]}
            self.report['workloads'].append(workload);self.save()
            print(f"IDLE {size} shipments: unchanged revisions={idle['unchanged_revisions']}",flush=True)
            for concurrency in (1,4,8):
                specs=[('operations','/v1/operations?limit=25',LARGE,None,None)]*self.args.requests
                wave,results=self.wave('operations-read',specs,concurrency)
                workload['waves'].append(wave);self.save()
                for sample,raw in results:
                    require(sample['status']==200,'Operations read failed')
                    data=json.loads(raw)
                    require(data['evidence_valid'] is True,'Operations evidence lost validity')
                    require(data['total']==fixtures['large']['counts']['cases'] and len(data['issues'])==25,'Operations response omitted expected work')
                    require(all(row['load_reference'].startswith(f'capacity-{size}-') for row in data['issues']),'Operations response crossed its tenant')
                require(wave['monitoring_valid'],'Resource monitoring failed')
            specs=[]
            for i in range(self.args.requests):
                specs.append((('workspace','/v1/workspace',LARGE,None,None) if i%4==0 else ('operations','/v1/operations?limit=25',LARGE,None,None)))
                specs.extend([('probe','/v1/operations?limit=25',PROBE,None,None),('health','/health',PROBE,None,None)])
            wave,results=self.wave('mixed-tenants-and-health',specs,8)
            workload['waves'].append(wave);self.save()
            for sample,raw in results:
                require(sample['status']==200,'Mixed request failed')
                data=json.loads(raw)
                if sample['kind']=='probe':require(all(r['load_reference'].startswith(f'probe-{size}-') for r in data['issues']),'Cross-tenant work leaked')
                if sample['kind']=='workspace':
                    require(len(data['loads'])==size,'Retained shipments missing')
                    require(all(data[key]==value for key,value in originals.items()),'Retained original physical evidence changed')
            load=f'capacity-{size}-00000';body={'action':'location','expected_version':1,'location':'Synthetic current observation'}
            specs=[('competing-version',f'/v1/loads/{load}/commands',LARGE,body,f'race-{i}') for i in range(8)]
            wave,results=self.wave('same-load-version-race',specs,8)
            workload['waves'].append(wave);self.save()
            require(sorted(s['status'] for s,_ in results)==[200]+[409]*7,'Competing requests did not commit exactly once')
            winner=next(i for i,(s,_) in enumerate(results) if s['status']==200)
            saved=self.snapshot();replay,raw=self.request(specs[winner]);require(replay['status']==200 and json.loads(raw)['replayed'] is True,'Lost-response replay failed')
            require([s['data_md5'] for s in saved]==[s['data_md5'] for s in self.snapshot()],'Replay changed retained records')
            specs=[('distinct-load-write',f'/v1/loads/capacity-{size}-{i:05}/commands',LARGE,{'action':'location','expected_version':1,'location':f'Synthetic concurrent observation {i}'},f'write-{i}') for i in range(1,5)]
            specs.append(('reconcile','/v1/operations/reconcile',LARGE,{},'explicit-reconciliation'))
            wave,results=self.wave('serialized-writes-and-reconciliation',specs,5)
            workload['waves'].append(wave);self.save()
            require(all(s['status']==200 for s,_ in results),'A noncompeting update was lost')
            versions=self.rows('freight_runtime',"SELECT l->>'id' AS id,(l->>'version')::int AS version,l->>'location' AS location FROM freight_workspaces,jsonb_array_elements(data->'loads') l WHERE tenant_id='large' AND l->>'id' IN ("+','.join(literal(f'capacity-{size}-{i:05}') for i in range(5))+')')
            require(len(versions)==5 and all(l['version']==2 for l in versions),'Committed versions were overwritten')
            require(all(l['location']==('Synthetic current observation' if l['id'].endswith('00000') else 'Synthetic concurrent observation '+str(int(l['id'].rsplit('-',1)[1]))) for l in versions),'Committed observations were overwritten')
            workload['write_verification']=versions
            workload['query_plan']=json.loads(self.sql('freight_runtime',"EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) SELECT data FROM freight_workspaces WHERE tenant_id='large'"))
            workload['storage_after']=self.snapshot()
            physical_after=self.physical_snapshot()
            require(physical_before==physical_after,'Concurrent writes changed original physical records')
            workload['retained_physical_records']={"before":physical_before,"after":physical_after,"unchanged":True}
            require(all(wave['monitoring_valid'] for wave in workload['waves']), 'Resource monitoring failed')
            self.stop('freight');self.save()
        self.report['qualification']={"functional_contracts":"pass","idle_workspaces_avoid_empty_writes":all(w['idle']['unchanged_revisions'] for w in self.report['workloads']),"capacity_sla":"not established","arrival_model":"finite bounded-client waves; client queue and response times reported separately"}

    def cleanup(self):
        try:
            if self.container:
                owner=self.command(['docker','inspect','--format','{{index .Config.Labels "com.stateset.freight.runtime"}}',self.container],label='diagnostic owner check')
                require(owner==self.container_label,'Refusing diagnostics on an unowned container')
                state=json.loads(self.command(['docker','inspect','--format','{{json .State}}',self.container],label='owned PostgreSQL state'))
                result=subprocess.run([shutil.which('docker') or 'docker','--host','unix:///var/run/docker.sock','logs',self.container],env={'PATH':os.defpath,'LC_ALL':'C'},stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,check=True)
                (self.directory/'postgres.log').write_text(self.safe(result.stdout))
                diagnostic={"state":state,"log_sha256":digest(result.stdout.encode()),"log":str(self.directory/'postgres.log')}
                self.report['postgres_diagnostics']=diagnostic
                try:
                    memory=self.command(['docker','exec',self.container,'cat','/sys/fs/cgroup/memory.events'],label='owned PostgreSQL memory events')
                    diagnostic['cgroup_version']=2
                    diagnostic['memory_events']=dict((key,int(value)) for key,value in (line.split() for line in memory.splitlines()))
                except RuntimeError:
                    diagnostic['cgroup_version']=1
                    for name in ['memory.oom_control','memory.failcnt','memory.max_usage_in_bytes','memory.limit_in_bytes','memory.stat']:
                        diagnostic[name]=self.command(['docker','exec',self.container,'cat','/sys/fs/cgroup/memory/'+name],label='owned PostgreSQL '+name)
        except Exception as error:
            self.report['diagnostic_error']=self.safe(str(error))
        finally:
            super().cleanup()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--baseline',type=Path)
    parser.add_argument('--sizes',type=lambda v:[int(x) for x in v.split(',')],default=[100,1000])
    parser.add_argument('--requests',type=int,default=16)
    parser.add_argument('--postgres-image',default='postgres:16')
    args=parser.parse_args()
    if args.output.exists():parser.error('preserve the original report; choose a new output path')
    if not args.sizes or any(n<10 or n>5000 for n in args.sizes) or not 8<=args.requests<=100:parser.error('sizes require 10–5000 shipments; requests require 8–100')
    runtime=Capacity(args)
    def interrupted(_signal,_frame):raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM,interrupted)
    try:
        runtime.build();runtime.postgres();runtime.checks()
        require(runtime.source==fingerprint(ROOT),'Application changed during qualification')
        require(all(digest((ROOT/p).read_bytes())==value for p,value in runtime.harness.items()),'Harness changed during qualification')
        runtime.report['result']='measured'
    except BaseException as error:
        runtime.report['result']='failed';runtime.report['error']=runtime.safe(f'{type(error).__name__}: {error}');print(runtime.report['error'],file=sys.stderr,flush=True)
    finally:
        try:runtime.cleanup()
        except BaseException as error:runtime.report.update(result='failed',cleanup_error=runtime.safe(str(error)))
        if runtime.report['result']=='measured':
            diagnostics=runtime.report.get('postgres_diagnostics',{})
            if diagnostics.get('cgroup_version')==2:
                kills=diagnostics.get('memory_events',{}).get('oom_kill')
            else:
                counters=dict(line.split() for line in diagnostics.get('memory.oom_control','').splitlines())
                kills=int(counters['oom_kill']) if 'oom_kill' in counters else None
            if runtime.report.get('diagnostic_error') or kills!=0:
                runtime.report.update(result='failed',error='PostgreSQL memory diagnostics unavailable or OOM kill observed')
            else:
                runtime.report['qualification']['no_database_oom_kills']=True
        runtime.report['finished_at']=dt.datetime.now(dt.timezone.utc).isoformat();runtime.save()
        args.output.parent.mkdir(parents=True,exist_ok=True)
        with args.output.open('x') as stream:stream.write((runtime.directory/'report.json').read_text())
        print('REPORT',args.output,flush=True)
    return 0 if runtime.report['result']=='measured' else 1


if __name__=='__main__':sys.exit(main())
