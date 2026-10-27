#!/usr/bin/env python3
"""Actual-service loss/restore drill with an original history cursor retained."""
import importlib.util
import json
from pathlib import Path
import shutil
from urllib.parse import urlencode

from runtime_contracts import ROOT, require

spec = importlib.util.spec_from_file_location("recovery", ROOT / "scripts/qualify-recovery.py")
recovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recovery)
BaseRecovery = recovery.Recovery


class IndexedHistoryRecovery(BaseRecovery):
    def __init__(self, args):
        super().__init__(args)
        self.capturing_history = False
        relative = "scripts/qualify-indexed-history-recovery.py"
        self.report["harness"][relative] = recovery.sha(Path(__file__))
        shutil.copyfile(Path(__file__), self.directory / Path(__file__).name)
        self.report["scope"] += "; original history cursor and portable index revision continuity"

    def backup_and_lose(self):
        self.history_before = self.http("freight", "/v1/history?kind=events&limit=2")
        require(self.history_before["next_cursor"], "History fixture must span pages")
        self.report["original_history_page"] = self.history_before
        self.capturing_history = True
        try:
            super().backup_and_lose()
        finally:
            self.capturing_history = False

    def interrupt(self, name):
        super().interrupt(name)
        if self.capturing_history and name == "freight":
            # API is quiesced; populate derived data before the common manifests
            # are captured. Neither source JSON nor its portable revision moves.
            original = self.raw_workspace()
            self.sql("freight_runtime", "SELECT freight_ensure_history('demo')")
            require(self.raw_workspace() == original, "Index warmup changed source")
            state = self.rows("freight_runtime", "SELECT w.history_revision,s.source_revision,s.ready FROM freight_workspaces w JOIN freight_history_state s USING(tenant_id) WHERE tenant_id='demo'")[0]
            require(state["ready"] and state["history_revision"] == state["source_revision"] and state["source_revision"] > 0, "Invalid portable checkpoint revision")
            self.report["history_index_checkpoint"] = state
            self.step("quiesced original history index and nonzero portable source revision enter the common backup", state=state)
            blocks = self.rows("freight_runtime", "SELECT count(*) AS blocks,coalesce(sum(jsonb_array_length(payload->'items')),0) AS originals FROM freight_workspace_blocks")[0]
            complete = self.rows("freight_runtime", "SELECT NOT EXISTS(SELECT 1 FROM freight_workspaces w CROSS JOIN LATERAL jsonb_object_keys(w.data->'frame'->'roots') name WHERE NOT freight_canonical_root_valid(w.tenant_id,w.data,name)) AND NOT EXISTS(SELECT 1 FROM freight_workspace_blocks b JOIN freight_workspaces w USING(tenant_id) WHERE NOT w.data->'frame'->'roots' ? b.root) AND NOT EXISTS(SELECT 1 FROM freight_workspaces WHERE data ? 'loads' OR data->'frame'->'blocks'<>'[]'::jsonb) AS valid")[0]["valid"]
            require(complete and blocks["blocks"]>0 and blocks["originals"]>0, "Canonical originals or membership metadata are incomplete")
            self.report["canonical_block_checkpoint"] = blocks
            self.step("canonical blocks and complete root membership enter the same three-service backup without an aggregate shadow", **blocks)

    def recovered_work(self):
        cursor = self.history_before["next_cursor"]
        captured = json.loads(bytes.fromhex(cursor))
        page = self.http("freight", "/v1/history?" + urlencode({"kind": "events", "limit": 2, "cursor": cursor}))
        expected = [{"position": n + 1, "record": event} for n, event in reversed(list(enumerate(self.before["events"][:captured["upper"]]))) if n + 1 < captured["before"]][:2]
        require(page["items"] == expected and page["captured_through"] == self.history_before["captured_through"] and page["total"] == self.history_before["total"], "Restored cursor changed original facts/window")
        require(page["tenant_id"] == "demo" and page["access"] == self.history_before["access"], "Restored history owner changed")
        self.report["restored_original_history_page"] = page
        self.step("original pre-loss history cursor resumes exact retained records after fresh-cluster restoration", originals=len(expected))
        super().recovered_work()


if __name__ == "__main__":
    recovery.Recovery = IndexedHistoryRecovery
    raise SystemExit(recovery.main())
