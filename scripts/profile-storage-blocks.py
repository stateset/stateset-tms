#!/usr/bin/env python3
"""Compare native Rust block packing against aggregate and record storage.

Owned experimental PostgreSQL only. No production migration or API changes.
Inputs are the same ten actually committed native outcomes as the preceding
record experiment. SQL, CLI codec and observer wall times are separate, with
production business/derived-index triggers excluded. Full reads are verified.
"""
import argparse
import datetime as dt
import hashlib
import importlib.util
import json
from pathlib import Path
import signal
import sys
import time

from runtime_contracts import ROOT, fingerprint, literal, require

spec=importlib.util.spec_from_file_location("record_probe",ROOT/"scripts/profile-storage-records.py")
previous=importlib.util.module_from_spec(spec);spec.loader.exec_module(previous)
encode,decode,sha=previous.encode,previous.decode,previous.sha
LAYOUTS={"packed16":16,"packed64":64}


def copy_text(value):
    return value.replace("\\","\\\\").replace("\t","\\t").replace("\n","\\n").replace("\r","\\r")


class Blocks(previous.Probe):
    def __init__(self,args):
        super().__init__(args)
        paths=["scripts/profile-storage-blocks.py","scripts/storage-blocks-prototype.sql"]
        paths += [str(p.relative_to(ROOT)) for p in (ROOT/"storage-codec").rglob("*") if p.is_file() and "target" not in p.parts]
        self.harness.update({p:sha((ROOT/p).read_bytes()) for p in paths})
        self.binary=args.codec_binary.resolve();require(self.binary.is_file(),"Build the native release codec first")
        self.binary_hash=sha(self.binary.read_bytes())
        self.report.update(scope=__doc__,native_codec={"path":str(self.binary),"sha256":self.binary_hash},
                           layouts=["aggregate","records",*LAYOUTS],workloads=[],representation_checks={})
        self.report["settings"].update(block_spans=list(LAYOUTS.values()),canonical_block_target_bytes=65536,
                                       oversized_original="retained as a singleton or scalar; not a universal read/memory bound")

    def codec(self,mode,value,span=64):
        require(sha(self.binary.read_bytes())==self.binary_hash,"Codec binary drift")
        command=[str(self.binary),mode]+([str(span),"65536"] if mode=="pack" else [])
        started=time.monotonic();raw=self.command(command,label="native codec "+mode,input=encode(value))
        return decode(raw),(time.monotonic()-started)*1000

    def setup(self):
        self.query((ROOT/"scripts/storage-records-prototype.sql").read_text(),"owned reference schemas")
        template=(ROOT/"scripts/storage-blocks-prototype.sql").read_text()
        for layout,span in LAYOUTS.items():
            self.query(template.replace("__LAYOUT__",layout).replace("__SPAN__",str(span)),"owned "+layout+" schema")

    def reset(self):
        names=["probe_aggregates","probe_heads"]+[p+"_heads" for p in LAYOUTS]
        self.query("TRUNCATE "+",".join(names)+" CASCADE;","isolate owned fixture relations")

    def seed_packed(self,tenant,layout,frame):
        require(frame["version"]==1 and frame["config"]["span"]==LAYOUTS[layout],"Wrong codec frame")
        owner=literal(tenant);names="ARRAY["+",".join(map(literal,frame["root_names"]))+"]::text[]"
        lines=["BEGIN;",f"INSERT INTO {layout}_heads VALUES({owner},0,18,1,{LAYOUTS[layout]},65536,{names});",
               f"COPY {layout}_roots(tenant_id,root,descriptor) FROM STDIN;"]
        lines += ["\t".join(map(copy_text,[tenant,root,encode(desc)])) for root,desc in frame["roots"].items()]
        lines += ["\\.",f"COPY {layout}_blocks(tenant_id,root,key,payload) FROM STDIN;"]
        lines += ["\t".join(map(copy_text,[tenant,b["root"],b["key"],encode(b)])) for b in frame["blocks"]]
        lines += ["\\.","COMMIT;"]
        self.query("\n".join(lines),"copy exact "+layout+" source")

    def packed_snapshot(self,tenant,layout):
        owner=literal(tenant);started=time.monotonic()
        raw=self.query("BEGIN ISOLATION LEVEL REPEATABLE READ;\n"
                       f"SELECT jsonb_build_array('head',format,revision,codec_version,span,target_bytes,root_names)::text FROM {layout}_heads WHERE tenant_id={owner} FOR SHARE;\n"
                       f"SELECT jsonb_build_array('root',root,descriptor,xmin::text,ctid::text)::text FROM {layout}_roots WHERE tenant_id={owner} ORDER BY root;\n"
                       f"SELECT jsonb_build_array('block',root,key,payload,xmin::text,ctid::text)::text FROM {layout}_blocks WHERE tenant_id={owner} ORDER BY root,key;\nCOMMIT;",
                       "stream "+layout+" complete original blocks")
        transport=(time.monotonic()-started)*1000
        lines=raw.splitlines();require(lines,"Missing packed header")
        head=decode(lines[0]);require(head[:2]==["head",18],"Missing/incompatible packed format")
        frame={"version":head[3],"config":{"span":head[4],"target_bytes":head[5]},"root_names":head[6],"roots":{},"blocks":[]}
        physical={}
        for line in lines[1:]:
            row=decode(line)
            if row[0]=="root":
                require(row[1] not in frame["roots"],"Duplicate packed root")
                frame["roots"][row[1]]=row[2];physical[("root",row[1])]=row[3:]
            else:
                require(row[0]=="block" and row[3]["root"]==row[1] and row[3]["key"]==row[2],"Invalid packed envelope")
                identity=("block",row[1],row[2]);require(identity not in physical,"Duplicate stored block")
                frame["blocks"].append(row[3]);physical[identity]=row[4:]
        value,codec_ms=self.codec("unpack",frame)
        require(value.get("_freight_format")==18,"Packed source marker mismatch")
        return value,physical,head[2],{"sql_transport_ms":transport,"codec_unpack_ms":codec_ms,
                                      "complete_probe_read_ms":(time.monotonic()-started)*1000,
                                      "stored_blocks":len(frame["blocks"]),"sql_text_bytes":len(raw.encode())},frame

    def packed_statement(self,tenant,layout,before,after,revision):
        owner=literal(tenant)
        roots={k:v for k,v in after["roots"].items() if k not in before["roots"] or encode(v)!=encode(before["roots"][k])}
        old={(b["root"],b["key"]):b for b in before["blocks"]};new={(b["root"],b["key"]):b for b in after["blocks"]}
        blocks={k:v for k,v in new.items() if k not in old or encode(v)!=encode(old[k])}
        removed=set(old)-set(new);removed_roots=set(before["roots"])-set(after["roots"])
        fence=f"DO $$ BEGIN PERFORM 1 FROM {layout}_heads WHERE tenant_id={owner} AND format=18 AND codec_version=1 AND revision={revision} FOR UPDATE; IF NOT FOUND THEN RAISE EXCEPTION 'stale packed revision'; END IF; END $$;\n"
        statements=[]
        if roots:
            values=",".join(f"({owner},{literal(root)},{literal(encode(v))}::jsonb)" for root,v in roots.items())
            statements.append(f"INSERT INTO {layout}_roots VALUES {values} ON CONFLICT(tenant_id,root) DO UPDATE SET descriptor=EXCLUDED.descriptor")
        if removed:
            keys=",".join(f"({literal(r)},{literal(k)})" for r,k in sorted(removed))
            statements.append(f"DELETE FROM {layout}_blocks WHERE tenant_id={owner} AND (root,key) IN ({keys})")
        if blocks:
            values=",".join(f"({owner},{literal(r)},{literal(k)},{literal(encode(v))}::jsonb)" for (r,k),v in blocks.items())
            statements.append(f"INSERT INTO {layout}_blocks VALUES {values} ON CONFLICT(tenant_id,root,key) DO UPDATE SET payload=EXCLUDED.payload")
        if removed_roots:
            statements.append(f"DELETE FROM {layout}_roots WHERE tenant_id={owner} AND root IN ("+",".join(map(literal,sorted(removed_roots)))+")")
        names="ARRAY["+",".join(map(literal,after["root_names"]))+"]::text[]"
        statements.append(f"UPDATE {layout}_heads SET revision=revision+1,root_names={names} WHERE tenant_id={owner}")
        changed={("root",r) for r in roots}|{("block",r,k) for r,k in blocks}|{("block",r,k) for r,k in removed}|{("root",r) for r in removed_roots}
        return fence,statements,{"changed_roots":len(roots),"changed_blocks":len(blocks),"deleted_blocks":len(removed),
                                 "canonical_block_payload_bytes":sum(len(encode(v).encode()) for v in blocks.values())},changed

    def all_footprints(self):
        names=["probe_aggregates","probe_heads","probe_roots","probe_items"]+[p+suffix for p in LAYOUTS for suffix in ["_heads","_roots","_blocks"]]
        rows=[decode(line) for line in self.query("SELECT jsonb_build_array(relname,pg_table_size(oid),pg_indexes_size(oid),pg_total_relation_size(oid))::text FROM pg_class WHERE relname IN ("+",".join(map(literal,names))+") ORDER BY relname;","all physical layout sizes").splitlines()]
        require(len(rows)==len(names),"Missing layout size")
        tables={r[0]:{"table_bytes":r[1],"index_bytes":r[2],"total_bytes":r[3]} for r in rows}
        totals={"aggregate":tables["probe_aggregates"]["total_bytes"],"records":sum(tables[p]["total_bytes"] for p in ["probe_heads","probe_roots","probe_items"])}
        totals.update({layout:sum(tables[layout+s]["total_bytes"] for s in ["_heads","_roots","_blocks"]) for layout in LAYOUTS})
        return {"relations":tables,"layout_total_bytes":totals,"scope":"Whole physical relations including TOAST/indexes; post-write figures include dead versions and background maintenance."}

    def integrity_checks(self):
        value={"_freight_format":18,"events":[{"id":"same","message":"x"*131072},{"id":"same","message":False}],
               "receipts":{"literal/\"雪{}":{"fingerprint":"original","response":[None,True,18446744073709551615,previous.Decimal("1.000000000000000000123456789")]}},
               "operations_cases":None,"opaque":{"$serde_json::private::Number":"retain"},"empty":[]}
        for layout,span in LAYOUTS.items():
            frame,_=self.codec("pack",value,span);self.seed_packed("edge",layout,frame);self.seed_packed("other",layout,frame)
            other,other_physical,other_revision,_,_=self.packed_snapshot("other",layout)
            actual,_,revision,_,_=self.packed_snapshot("edge",layout);require(encode(actual)==encode(value) and revision==0,"Oversized/literal/numeric original changed")
            after=decode(encode(value));after["events"].reverse();after["operations_cases"]={"future":[False,None]};after["new root"]={"é":1,"e\u0301":False};del after["empty"]
            next_frame,_=self.codec("pack",after,span);fence,statements,_,_=self.packed_statement("edge",layout,frame,next_frame,0);self.execute(fence,statements,"edge-"+layout)
            actual,physical,revision,_,_=self.packed_snapshot("edge",layout);require(encode(actual)==encode(after) and revision==1,"Shape/order transition lost originals")
            unchanged,p,r,_,_=self.packed_snapshot("other",layout);require(encode(unchanged)==encode(other) and p==other_physical and r==other_revision,"Another tenant changed")
            try:self.execute(fence,statements,"edge-stale-"+layout)
            except RuntimeError as error:require("stale packed revision" in str(error),"Unexpected stale refusal")
            else:raise AssertionError("Stale packed write succeeded")
            actual2,p,r,_,_=self.packed_snapshot("edge",layout);require(encode(actual2)==encode(after) and p==physical and r==revision,"Stale packed mutation left partial writes")
            selected=next(b for b in next_frame["blocks"] if b["root"]=="events")
            self.query(f"DELETE FROM {layout}_blocks WHERE tenant_id='edge' AND root='events' AND key="+literal(selected["key"]),"inject missing packed block")
            try:self.packed_snapshot("edge",layout)
            except RuntimeError as error:require("array ordinal" in str(error),"Unexpected missing block refusal")
            else:raise AssertionError("Missing packed original accepted")
            self.query(f"INSERT INTO {layout}_blocks VALUES('edge','events',"+literal(selected["key"])+","+literal(encode(selected))+"::jsonb)","restore packed original")
            self.query(f"DELETE FROM {layout}_roots WHERE tenant_id='edge' AND root='operations_cases'","inject missing packed root")
            try:self.packed_snapshot("edge",layout)
            except RuntimeError as error:require("root manifest" in str(error),"Unexpected missing root refusal")
            else:raise AssertionError("Missing packed root defaulted")
        self.report["representation_checks"]={"exact_numbers_and_literal_keys":True,"private_looking_keys_opaque":True,
            "oversized_original_preserved":True,"duplicate_ids_order_shapes_and_unicode_forms":True,
            "cross_tenant_rows_unchanged":True,"stale_revision_atomic_refusal":True,"missing_block_refused":True,"missing_root_refused":True}
        self.reset()

    def checks(self):
        self.setup();self.integrity_checks()
        native_path=self.args.native_report.resolve();native=json.loads(native_path.read_text())
        require(native["result"]=="measured" and native["no_oom_kills"] and native["builds"]["freight"]["source"]==self.source,"Native source not qualified/current")
        self.report["native_report"]={"path":str(native_path),"sha256":sha(native_path.read_bytes())}
        for w in native["workloads"]:
            self.reset();tenant="blocks-"+str(w["shipments"]);initial=self.archived(w["samples"][0]["source_before"])
            self.seed(tenant,initial);frames={};physical={};seed_pack_ms={}
            for layout,span in LAYOUTS.items():
                frame,ms=self.codec("pack",initial,span);frames[layout]=frame;seed_pack_ms[layout]=ms;self.seed_packed(tenant,layout,frame)
                value,p,revision,_,_=self.packed_snapshot(tenant,layout);require(encode(value)==encode(initial) and revision==0,"Packed backfill changed originals");physical[layout]=p
            _,physical["records"],_=self.snapshot(tenant)
            output={"shipments":w["shipments"],"history_depth":w["history_depth"],"initial_storage":self.all_footprints(),"initial_codec_pack_ms":seed_pack_ms,
                    "initial_block_counts":{p:len(f["blocks"]) for p,f in frames.items()},"samples":[]}
            self.report["workloads"].append(output)
            for n,sample in enumerate(w["samples"]):
                before,after=self.archived(sample["source_before"]),self.archived(sample["source_after"])
                packed_after={};pack_ms={};mutations={};reads={};preserved={}
                for layout,span in LAYOUTS.items():packed_after[layout],pack_ms[layout]=self.codec("pack",after,span)
                plans={};changed={}
                aggregate="UPDATE probe_aggregates SET data="+literal(encode(after))+"::jsonb,revision=revision+1 WHERE tenant_id="+literal(tenant)+" AND revision="+str(n)
                plans["aggregate"]=("",[aggregate],{"complete_payload_bytes":len(encode(after).encode())})
                fence,statements,counts=self.records_statement(tenant,before,after,n);plans["records"]=(fence,statements,counts)
                cr,ci,dr,di=self.delta(before,after);changed["records"]={(r,) for r in cr}|set(ci)|set(di)|{(r,) for r in dr}
                for layout in LAYOUTS:
                    fence,statements,counts,changed[layout]=self.packed_statement(tenant,layout,frames[layout],packed_after[layout],n);plans[layout]=(fence,statements,counts)
                names=["aggregate","records",*LAYOUTS];order=names[n%len(names):]+names[:n%len(names)]
                for mode in order:
                    fence,statements,counts=plans[mode];mutations[mode]={**counts,**self.execute(fence,statements,tenant+"-"+str(n)+"-"+mode)}
                start=time.monotonic();row=decode(self.query("SELECT jsonb_build_array(data,revision)::text FROM probe_aggregates WHERE tenant_id="+literal(tenant),"complete aggregate read"));require(encode(row[0])==encode(after) and row[1]==n+1,"Aggregate source diverged");reads["aggregate"]={"complete_probe_read_ms":(time.monotonic()-start)*1000}
                start=time.monotonic();value,p,revision=self.snapshot(tenant);require(encode(value)==encode(after) and revision==n+1,"Record source diverged");reads["records"]={"complete_probe_read_ms":(time.monotonic()-start)*1000};untouched=set(physical["records"])-changed["records"];require(all(p[k]==physical["records"][k] for k in untouched),"Unrelated record tuple changed");preserved["records"]=len(untouched);physical["records"]=p
                complete={}
                for layout in LAYOUTS:
                    value,p,revision,stats,frame=self.packed_snapshot(tenant,layout);require(encode(value)==encode(after) and revision==n+1,"Packed source diverged")
                    untouched=set(physical[layout])-changed[layout];require(all(p[k]==physical[layout][k] for k in untouched),"Unrelated block tuple changed");preserved[layout]=len(untouched)
                    prefix="o/"+hashlib.sha256(sample["key"].encode()).hexdigest()[:2]+"/"
                    owner=literal(tenant);receipt=self.query(f"BEGIN; SELECT format FROM {layout}_heads WHERE tenant_id={owner} FOR SHARE; SELECT e->'payload' FROM {layout}_blocks b CROSS JOIN LATERAL jsonb_array_elements(b.payload->'items') e WHERE b.tenant_id={owner} AND b.root='receipts' AND b.key>="+literal(prefix)+" AND b.key<"+literal(prefix+"~")+" AND e->>'name'="+literal(sample["key"])+" AND e->>'position'='0'; COMMIT;","selected literal packed receipt")
                    rows=receipt.splitlines();require(rows[0]=="18" and len(rows)==2 and encode(decode(rows[1]))==encode(after["receipts"][sample["key"]]),"Packed receipt changed")
                    replay,rp,rr,_,_=self.packed_snapshot(tenant,layout);require(encode(replay)==encode(after) and rp==p and rr==revision,"Packed receipt lookup wrote source")
                    reads[layout]=stats;physical[layout]=p;frames[layout]=packed_after[layout];complete[layout]=self.capture(tenant+"-"+str(n)+"-"+layout+"-source",value)
                output["samples"].append({"order":order,"mutations":mutations,"full_reads":reads,"codec_pack_ms":pack_ms,"unchanged_physical_rows":preserved,"exact_read_only_receipts":True,"native_before":sample["source_before"],"native_after":sample["source_after"],"complete_sources":complete})
                print("VERIFIED",tenant,n,{p:mutations[p]["canonical_block_payload_bytes"] for p in LAYOUTS},flush=True);self.save()
            output["post_mutation_storage"]=self.all_footprints()
        require(sha(self.binary.read_bytes())==self.binary_hash,"Final native codec drift")
        self.step("all native outcomes, full reads and original receipts survive packed PostgreSQL storage")


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--output",type=Path,required=True);parser.add_argument("--codec-binary",type=Path,required=True);parser.add_argument("--native-report",type=Path,default=ROOT/"docs/evidence/prepared-write-after-confirmation.json");parser.add_argument("--postgres-image",default="postgres:16");args=parser.parse_args()
    if args.output.exists():parser.error("Preserve evidence; select a new output path")
    run=Blocks(args);signal.signal(signal.SIGTERM,lambda *_:(_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run.postgres();run.prepare_sampling();run.sql("postgres","CREATE DATABASE storage_blocks");run.url=f"postgres://freight_runtime@127.0.0.1:{run.ports['postgres']}/storage_blocks";run.checks()
        require(fingerprint(ROOT)==run.source,"Production core drift");require(all(sha((ROOT/p).read_bytes())==h for p,h in run.harness.items()),"Experiment source drift");run.report["result"]="measured"
    except BaseException as error:run.report.update(result="failed",error=run.safe(f"{type(error).__name__}: {error}"));print(run.report["error"],file=sys.stderr,flush=True)
    finally:
        try:run.stop_sampling()
        except BaseException as error:run.report.update(result="failed",sampling_error=run.safe(str(error)))
        run.trace_enabled=False
        try:run.cleanup()
        except BaseException as error:run.report.update(result="failed",cleanup_error=run.safe(str(error)))
        d=run.report.get("postgres_diagnostics",{})
        if d.get("cgroup_version")==2:kills=d.get("memory_events",{}).get("oom_kill");hits=d.get("memory_events",{}).get("max")
        else:
            counters=dict(line.split() for line in d.get("memory.oom_control","").splitlines());kills=int(counters["oom_kill"]) if "oom_kill" in counters else None;hits=int(d["memory.failcnt"]) if "memory.failcnt" in d else None
        run.report["no_oom_or_limit_hits"]=kills==hits==0 and d.get("state",{}).get("OOMKilled") is False
        if not run.report["no_oom_or_limit_hits"] or run.report.get("diagnostic_error"):run.report.update(result="failed",resource_error="OOM, limit hit or missing diagnostics")
        run.report["finished_at"]=dt.datetime.now(dt.timezone.utc).isoformat();run.save()
        with args.output.open("x") as f:f.write((run.directory/"report.json").read_text())
        print("REPORT",args.output,flush=True)
    return 0 if run.report["result"]=="measured" else 1


if __name__=="__main__":sys.exit(main())
