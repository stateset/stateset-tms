import {
  saveJournal,
  journalOwner,
  expectedAccess,
  JOURNAL_CHANNEL,
} from "../lib/save-journal.mjs";
import { RateConfirmations } from "../components/RateConfirmations";
import { CommerceTracking } from "../components/CommerceTracking";
import { CommerceHandoffReview } from "../components/CommerceHandoffReview";
import { SourceControl, SourceQueue } from "../components/SourceControl";
import { Modal } from "../components/Modal";
import dynamic from "next/dynamic";
import { useFreightIdentity } from "../components/FreightIdentity";
import { publicAuthConfig } from "../lib/auth-config.mjs";
import { loadBoardCsv } from "../lib/load-board-csv.mjs";
import {
  currencyCode,
  currencySet,
  formatMoney,
  formatTotals,
  moneyInput,
  parseMoney,
  summaryTotals,
} from "../lib/money.mjs";
import { SaveRecovery } from "../components/SaveRecovery";
const ClerkWorkspace = dynamic(() => import("../components/ClerkWorkspace"), {
  ssr: false,
});
import { TenderHistory } from "../components/TenderHistory";
import { IntermediateStopsEditor } from "../components/IntermediateStopsEditor";
import { ItineraryExecution } from "../components/ItineraryExecution";
import { StopExecution } from "../components/StopExecution";
import { RecordHistory } from "../components/RecordHistory";
import { ExecutionTimeline } from "../components/ExecutionTimeline";
import { MovementCorrections } from "../components/MovementCorrections";
import { OperationsWorkspace } from "../components/OperationsWorkspace";
import { ShipmentCharges } from "../components/ShipmentCharges";
import { CancellationFees } from "../components/CancellationFees";
import { QuoteWorkspace } from "../components/QuoteWorkspace";
import { CarrierContracts } from "../components/CarrierContracts";
import { CarrierPayables } from "../components/CarrierPayables";
import { FinanceWorkspace } from "../components/FinanceWorkspace";
import { InvoiceApprovalEvidence } from "../components/InvoiceApprovalEvidence";
import Head from "next/head";
import { ShipmentDocuments } from "../components/ShipmentDocuments";
import {
  InboundDocuments,
  InboundDocumentDetail,
} from "../components/InboundDocuments";
import {
  createContext,
  useContext,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import {
  Activity,
  ShieldAlert,
  ArrowRight,
  ArrowUpRight,
  Bell,
  Box,
  Building2,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Circle,
  DollarSign,
  Download,
  ExternalLink,
  FileCheck2,
  LayoutDashboard,
  Link2,
  Loader2,
  LogOut,
  MapPin,
  Menu,
  Package,
  Plus,
  RefreshCw,
  Search,
  ShieldCheck,
  Truck,
  Users,
  Wallet,
  X,
  AlertTriangle,
} from "lucide-react";

const cash = (value, currency) => formatMoney(value, currency);
// Imported orders suggest their own currency; a malformed code falls back to
// USD, matching the API's import default.
const orderCurrency = (order) => {
  try {
    return currencyCode(order?.currency);
  } catch {
    return "USD";
  }
};
const buyTotal = (load) =>
  load.commercial?.total_buy_cents ??
  load.commercial?.buy?.total_amount_cents ??
  load.buy_cents;
const sellTotal = (load) =>
  load.commercial?.total_sell_cents ??
  load.commercial?.sell?.total_amount_cents ??
  load.sell_cents;
const label = (value) =>
  (value || "").replaceAll("_", " ").replace(/\b\w/g, (v) => v.toUpperCase());
const date = (value) =>
  new Date(`${value}T12:00:00`).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
  });
const tabs = [
  ["overview", "Overview", LayoutDashboard],
  ["operations", "Control tower", ShieldAlert],
  ["loads", "Load board", Package],
  ["quotes", "Quotes", FileCheck2],
  ["tracking", "Tracking", MapPin],
  ["dispatch", "Dispatch", Truck],
  ["carriers", "Carrier network", ShieldCheck],
  ["customers", "Customers", Building2],
  ["finance", "Freight audit", Wallet],
  ["integrations", "Integrations", Link2],
  ["activity", "Activity log", Activity],
];
async function apiRequest(fetcher, path, body, key, owner) {
  const approvalRead =
    body === undefined && /^invoices\/[^/]+\/approval-evidence$/.test(path);
  const response = await fetcher(`/api/freight/${path}`, {
    ...(body === undefined
      ? {
          headers: {
            ...(path === "workspace?history=deferred"
              ? { "X-Freight-Approval-Details": "deferred" }
              : {}),
            ...((path.startsWith("history?") || approvalRead) && owner
              ? { "X-Freight-Expected-Access": expectedAccess(owner) }
              : {}),
          },
        }
      : {}),
    ...(approvalRead ? { signal: AbortSignal.timeout(15000) } : {}),
    ...(path === "save-journal" ? { signal: AbortSignal.timeout(15000) } : {}),
    ...(body !== undefined
      ? {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "Idempotency-Key": key || crypto.randomUUID(),
            ...(owner
              ? { "X-Freight-Expected-Access": expectedAccess(owner) }
              : {}),
          },
          body: JSON.stringify(body),
        }
      : {}),
  });
  const data = await response.json();
  if (!response.ok) {
    const error = new Error(
      data.error?.message || `Request failed (${response.status})`,
    );
    error.status = response.status;
    error.code = data.error?.code;
    throw error;
  }
  return data;
}
function Badge({ value, danger }) {
  return (
    <span className={`badge ${danger ? "exception" : value}`}>
      {danger ? <AlertTriangle size={11} /> : <span className="badge-dot" />}
      {danger ? "Exception" : label(value)}
    </span>
  );
}
function RecoveryStorage({ problem, busy, retry }) {
  return (
    <section className="save-recovery" aria-label="Recovery storage">
      <p role="alert">{problem}</p>
      <button className="secondary" disabled={busy} onClick={retry}>
        Retry recovery storage
      </button>
    </section>
  );
}
function Empty({ title, children }) {
  return (
    <div className="empty">
      <Package size={28} />
      <h3>{title}</h3>
      <p>{children || "Your workspace is ready for its next move."}</p>
    </div>
  );
}
const RecordDefaults = createContext({});
function fieldValues(record) {
  if (!record) return {};
  if (!record.origin) return record;
  return {
    ...record,
    origin_facility: record.origin.facility,
    origin_city: record.origin.city,
    origin_state: record.origin.state,
    pickup: record.origin.date,
    origin_address: record.origin.address_lines?.[0] || "",
    origin_address_2: record.origin.address_lines?.[1] || "",
    origin_postal: record.origin.postal_code || "",
    origin_country: record.origin.country || "US",
    origin_contact: record.origin.contact || "",
    origin_phone: record.origin.phone || "",
    origin_instructions: record.origin.instructions || "",
    destination_facility: record.destination.facility,
    destination_city: record.destination.city,
    destination_state: record.destination.state,
    delivery: record.destination.date,
    destination_address: record.destination.address_lines?.[0] || "",
    destination_address_2: record.destination.address_lines?.[1] || "",
    destination_postal: record.destination.postal_code || "",
    destination_country: record.destination.country || "US",
    destination_contact: record.destination.contact || "",
    destination_phone: record.destination.phone || "",
    destination_instructions: record.destination.instructions || "",
    currency: currencyCode(record.currency),
    buy: moneyInput(record.buy_cents, record.currency),
    sell: moneyInput(record.sell_cents, record.currency),
  };
}
function Field({ name, title, type = "text", children, ...props }) {
  const defaults = useContext(RecordDefaults);
  props = { ...props, defaultValue: defaults[name] ?? props.defaultValue };
  return (
    <label className="field">
      <span>{title}</span>
      {children ? (
        <select name={name} aria-label={title} {...props}>
          {children}
        </select>
      ) : (
        <input name={name} aria-label={title} type={type} {...props} />
      )}
    </label>
  );
}

export async function getServerSideProps({ res }) {
  res.setHeader("Cache-Control", "no-store");
  return { props: { authConfig: publicAuthConfig() } };
}
export default function FreightPage({ authConfig }) {
  return authConfig?.mode === "clerk" ? (
    <ClerkWorkspace publishableKey={authConfig.publishableKey}>
      <Freight />
    </ClerkWorkspace>
  ) : (
    <Freight />
  );
}
function Freight() {
  const identity = useFreightIdentity();
  const sessionEpoch = useRef(0),
    refreshSequence = useRef(0),
    accessSuspended = useRef(false);
  const pendingMutation = useRef(null),
    mutationRunning = useRef(false),
    mutationEpoch = useRef(0),
    recoveryRef = useRef(null);
  const journalScope = useRef(null),
    forgetScope = useRef(null),
    forgetPending = useRef(Promise.resolve());
  const [journalProblem, setJournalProblem] = useState("");
  const [saveRecovery, setSaveRecovery] = useState(null);
  const updateRecovery = useCallback((value) => {
    recoveryRef.current = value;
    setSaveRecovery(value);
  }, []);
  const adoptJournal = useCallback(
    (record) => {
      pendingMutation.current = record.attempt;
      updateRecovery(
        record.attempt
          ? {
              ...record.attempt,
              message:
                record.attempt.kind === "stopped"
                  ? "Original save stopped before application"
                  : record.attempt.message,
              restored: true,
            }
          : null,
      );
    },
    [updateRecovery],
  );
  const [podChoice, setPodChoice] = useState("");
  const [financeSection, setFinanceSection] = useState("carrier_audit");
  const [quoteSelection, setQuoteSelection] = useState(null);
  const [workspace, setWorkspace] = useState(null),
    [page, setPage] = useState("overview"),
    [search, setSearch] = useState(""),
    [filter, setFilter] = useState("all"),
    [role, setRole] = useState("All operations");
  const [error, setError] = useState(""),
    [auth, setAuth] = useState(false),
    [loading, setLoading] = useState(true),
    [busy, setBusy] = useState(false),
    [signingOut, setSigningOut] = useState(false),
    [signOutFailed, setSignOutFailed] = useState(false),
    [toast, setToast] = useState("");
  const [modal, setModal] = useState(null),
    [selected, setSelected] = useState(null),
    [mobile, setMobile] = useState(false),
    [orders, setOrders] = useState(null),
    [orderPage, setOrderPage] = useState(1);
  const catalogLinkOpened = useRef(false);
  useEffect(() => {
    if (!workspace || catalogLinkOpened.current) return;
    catalogLinkOpened.current = true;
    const loadId = new URLSearchParams(window.location.search).get("load");
    if (!loadId) return;
    if (workspace.loads.some((load) => load.id === loadId)) {
      setSelected(loadId);
      setModal({ kind: "detail" });
    } else setError("The requested shipment is unavailable in this workspace.");
  }, [workspace]);
  const clearAccess = useCallback(
    (message = "", forget = true) => {
      const scope = journalScope.current;
      journalScope.current = null;
      if (scope && forget) {
        forgetScope.current = scope;
        forgetPending.current = saveJournal(scope.owner, "forget", scope);
        forgetPending.current.catch(() => {});
      }
      setJournalProblem("");
      accessSuspended.current = true;
      sessionEpoch.current++;
      refreshSequence.current++;
      pendingMutation.current = null;
      mutationRunning.current = false;
      updateRecovery(null);
      setAuth(true);
      setWorkspace(null);
      setModal(null);
      setSelected(null);
      setOrders(null);
      setOrderPage(1);
      setQuoteSelection(null);
      setPodChoice("");
      setToast("");
      setSearch("");
      setFilter("all");
      setPage("overview");
      setMobile(false);
      setLoading(false);
      setBusy(false);
      setError(message);
    },
    [updateRecovery],
  );
  const request = useCallback(
    async (...args) => {
      const epoch = sessionEpoch.current;
      const changed = () =>
        Object.assign(new Error("This workspace session has changed."), {
          sessionChanged: true,
        });
      if (accessSuspended.current) throw changed();
      try {
        const result = await apiRequest(
          identity.fetch,
          args[0],
          args[1],
          args[2],
          args[3] ?? journalScope.current?.owner,
        );
        if (epoch !== sessionEpoch.current || accessSuspended.current)
          throw changed();
        return result;
      } catch (e) {
        if (
          e.code === "workspace_session_changed" &&
          epoch === sessionEpoch.current &&
          !accessSuspended.current
        )
          clearAccess(e.message, false);
        if (epoch !== sessionEpoch.current || accessSuspended.current)
          throw changed();
        throw e;
      }
    },
    [identity.fetch, clearAccess],
  );
  const readHistory = useCallback(
    async (path, owner) => {
      const epoch = sessionEpoch.current;
      try {
        const data = await request(path, undefined, undefined, owner);
        if (epoch !== sessionEpoch.current || accessSuspended.current)
          throw Object.assign(new Error("Workspace session changed"), {
            sessionChanged: true,
          });
        const [, , tenant, subject] = JSON.parse(owner);
        if (data.tenant_id !== tenant || data.access?.subject !== subject) {
          clearAccess(
            "Your workspace changed. Sign in again to view history.",
            false,
          );
          throw Object.assign(new Error("Workspace session changed"), {
            sessionChanged: true,
          });
        }
        return data;
      } catch (error) {
        if (
          epoch === sessionEpoch.current &&
          (error.status === 401 || error.status === 403)
        )
          clearAccess(error.message, false);
        throw error;
      }
    },
    [request, clearAccess],
  );
  const journal = useCallback(
    (owner, operation, options = {}) =>
      saveJournal(owner, operation, {
        ...options,
        codec: async (action, verifiedOwner, value) =>
          (
            await request("save-journal", {
              action,
              owner: verifiedOwner,
              [action === "seal" ? "intent" : "sealed"]: value,
            })
          ).data,
      }),
    [request],
  );
  const load = useCallback(
    async (quiet = false) => {
      if (accessSuspended.current) return false;
      const generation = ++refreshSequence.current;
      if (!quiet) setLoading(true);
      const epoch = sessionEpoch.current;
      try {
        if (!journalScope.current) {
          const access = await request("access");
          const owner = journalOwner(
            access,
            identity.mode,
            identity.organization,
            identity.session,
          );
          try {
            const record = await journal(owner, "read", {
              onGeneration: (retainedGeneration) => {
                if (
                  epoch === sessionEpoch.current &&
                  generation === refreshSequence.current
                )
                  if (
                    journalScope.current &&
                    (journalScope.current.owner !== owner ||
                      journalScope.current.generation !== retainedGeneration)
                  )
                    clearAccess(
                      "This workspace session has changed. Sign in again before recovery.",
                      false,
                    );
                  else
                    journalScope.current = {
                      owner,
                      generation: retainedGeneration,
                    };
              },
            });
            if (
              epoch !== sessionEpoch.current ||
              generation !== refreshSequence.current
            )
              return false;
            journalScope.current = { owner, generation: record.generation };
            adoptJournal(record);
            setJournalProblem("");
          } catch (problem) {
            if (epoch !== sessionEpoch.current) return false;
            setJournalProblem(problem.message);
          }
        }
        const scope = journalScope.current;
        const completedKey =
          recoveryRef.current && recoveryRef.current.kind !== "unknown"
            ? pendingMutation.current?.key
            : null;
        const data = await request("workspace?history=deferred");
        if (generation !== refreshSequence.current) return false;
        if (
          scope &&
          journalOwner(
            data,
            identity.mode,
            identity.organization,
            identity.session,
          ) !== scope.owner
        ) {
          clearAccess(
            "This workspace session has changed. Sign in again before saving.",
            false,
          );
          return false;
        }
        if (scope && (!mutationRunning.current || completedKey)) {
          try {
            let record = await journal(scope.owner, "read");
            if (
              epoch !== sessionEpoch.current ||
              generation !== refreshSequence.current
            )
              return false;
            if (record.generation !== scope.generation) {
              clearAccess(
                "This workspace session has changed. Sign in again before saving.",
                false,
              );
              return false;
            }
            if (
              completedKey &&
              record.attempt?.key === completedKey &&
              record.attempt.kind !== "unknown"
            ) {
              setToast(
                record.attempt.kind === "stopped"
                  ? "Original save stopped before application"
                  : record.attempt.message || "Changes saved",
              );
              record = await journal(scope.owner, "remove", {
                ...scope,
                key: completedKey,
              });
              if (
                epoch !== sessionEpoch.current ||
                generation !== refreshSequence.current
              )
                return false;
            }
            adoptJournal(record);
            setJournalProblem("");
          } catch (problem) {
            if (epoch !== sessionEpoch.current) return false;
            setJournalProblem(problem.message);
          }
        }
        setWorkspace(data);
        setAuth(false);
        setError("");
        return true;
      } catch (e) {
        if (generation !== refreshSequence.current || e.sessionChanged)
          return false;
        if (e.status === 401 || e.status === 403) {
          clearAccess(identity.mode === "clerk" ? e.message : "");
        } else setError(e.message);
        return false;
      } finally {
        if (generation === refreshSequence.current) setLoading(false);
      }
    },
    [
      request,
      identity.mode,
      identity.organization,
      identity.session,
      clearAccess,
      adoptJournal,
      journal,
    ],
  );
  useEffect(() => {
    load();
    const timer = setInterval(() => load(true), 30000);
    return () => {
      sessionEpoch.current++;
      refreshSequence.current++;
      clearInterval(timer);
    };
  }, [load]);
  useEffect(() => {
    if (typeof BroadcastChannel === "undefined") return;
    let channel;
    try {
      channel = new BroadcastChannel(JOURNAL_CHANNEL);
    } catch {
      return;
    }
    channel.onmessage = async ({ data }) => {
      const scope = journalScope.current,
        epoch = sessionEpoch.current,
        mutation = mutationEpoch.current,
        wasMutating = mutationRunning.current;
      if (!scope || data?.owner !== scope.owner || accessSuspended.current)
        return;
      try {
        const record = await journal(scope.owner, "read");
        if (epoch !== sessionEpoch.current) return;
        if (record.generation !== scope.generation) {
          clearAccess("This workspace was signed out in another tab.", false);
        } else if (
          !wasMutating &&
          !mutationRunning.current &&
          mutation === mutationEpoch.current
        )
          adoptJournal(record);
      } catch (problem) {
        if (epoch === sessionEpoch.current) setJournalProblem(problem.message);
      }
    };
    return () => channel.close();
  }, [clearAccess, adoptJournal, journal]);
  async function signOut() {
    clearAccess();
    const epoch = sessionEpoch.current;
    setSignOutFailed(false);
    setSigningOut(true);
    setBusy(true);
    try {
      const cleanup = forgetPending.current
        .catch(() => {
          const scope = forgetScope.current;
          return scope ? saveJournal(scope.owner, "forget", scope) : undefined;
        })
        .then(
          () => true,
          () => false,
        );
      let identityResult;
      if (identity.signOut) identityResult = await identity.signOut();
      else {
        const response = await fetch("/api/session", { method: "DELETE" });
        if (!response.ok) throw new Error("Sign-out request failed");
      }
      const recoveryCleared = await cleanup;
      if (epoch !== sessionEpoch.current) return;
      if (!recoveryCleared || identityResult?.recoveryCleared === false) {
        setSignOutFailed(true);
        setError(
          "You are signed out. Browser save recovery could not be cleared. Retry sign out when browser storage is available.",
        );
      }
    } catch {
      if (epoch !== sessionEpoch.current) return;
      setSignOutFailed(true);
      setError(
        "Your local workspace is closed. Sign out could not be confirmed. Retry sign out.",
      );
    } finally {
      if (epoch === sessionEpoch.current) {
        setSigningOut(false);
        setBusy(false);
      }
    }
  }
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(""), 4500);
    return () => clearTimeout(timer);
  }, [toast]);
  const navigate = (target) => {
    setPage(target);
    setSearch("");
    setFilter("all");
    setMobile(false);
  };
  const perform = async (path, body, message, replay = false) => {
    if (
      accessSuspended.current ||
      mutationRunning.current ||
      (recoveryRef.current && !replay)
    )
      return;
    const epoch = sessionEpoch.current;
    mutationRunning.current = true;
    mutationEpoch.current++;
    setBusy(true);
    setError("");
    let attempt,
      sent = false;
    const scope = journalScope.current;
    try {
      if (!scope)
        throw new Error(
          "Browser save recovery is not ready. Refresh the workspace before saving.",
        );
      if (replay) {
        const recoveringKey = pendingMutation.current?.key;
        const record = await journal(scope.owner, "check", {
          ...scope,
          key: recoveringKey,
        });
        if (epoch !== sessionEpoch.current) return;
        if (record.generation !== scope.generation) {
          clearAccess("This workspace session has changed.", false);
          return;
        }
        adoptJournal(record);
        attempt = record.attempt;
        if (
          !attempt ||
          attempt.key !== recoveringKey ||
          attempt.kind !== "unknown"
        ) {
          await load(true);
          return;
        }
      } else {
        const fingerprint = JSON.stringify([path, body]);
        const [savedPath, savedBody] = JSON.parse(fingerprint);
        const record = await journal(scope.owner, "claim", {
          ...scope,
          attempt: {
            fingerprint,
            key: crypto.randomUUID(),
            path: savedPath,
            body: savedBody,
            message,
            kind: "unknown",
            checks: 0,
            createdAt: new Date().toISOString(),
          },
        });
        if (epoch !== sessionEpoch.current) return;
        if (!record.claimed) {
          adoptJournal(record);
          setError(
            "Another original save needs recovery before a new save can be sent.",
          );
          return;
        }
        attempt = record.attempt;
        pendingMutation.current = attempt;
      }
      sent = true;
      await request(attempt.path, attempt.body, attempt.key);
      if (epoch !== sessionEpoch.current) return;
      const saved = await journal(scope.owner, "confirmed", {
        ...scope,
        key: attempt.key,
      });
      if (epoch !== sessionEpoch.current) return;
      adoptJournal(saved);
      setModal(null);
      await load(true);
    } catch (e) {
      if (epoch !== sessionEpoch.current || e.sessionChanged) return;
      if (!sent) {
        setJournalProblem(e.message);
        return;
      }
      if (e.code === "request_stopped") {
        try {
          const stopped = await journal(scope.owner, "stopped", {
            ...scope,
            key: attempt.key,
          });
          if (epoch !== sessionEpoch.current) return;
          adoptJournal(stopped);
        } catch (problem) {
          setJournalProblem(problem.message);
        }
        setModal(null);
        await load(true);
        return;
      }
      // A missing response or server/proxy failure cannot establish whether the
      // original transaction committed. Even a rejected replay leaves its
      // earlier outcome unknown until an original successful receipt returns.
      if (
        replay ||
        !e.status ||
        e.status >= 500 ||
        e.status === 408 ||
        e.status === 429
      ) {
        updateRecovery({ ...attempt, kind: "unknown" });
        setError(e.message);
        return;
      }
      try {
        const rejected = await journal(scope.owner, "remove", {
          ...scope,
          key: attempt.key,
          expectedChecks: attempt.checks,
        });
        if (epoch !== sessionEpoch.current) return;
        adoptJournal(rejected);
      } catch (problem) {
        setJournalProblem(problem.message);
        return;
      }
      if (
        e.message.includes("Sync source") ||
        (e.status === 409 && modal?.kind === "command")
      )
        await load(true);
      if (epoch !== sessionEpoch.current) return;
      setError(e.message);
    } finally {
      if (epoch === sessionEpoch.current) {
        mutationRunning.current = false;
        setBusy(false);
      }
    }
  };
  const recoverSave = async () => {
    if (accessSuspended.current || mutationRunning.current) return;
    if (recoveryRef.current?.kind === "unknown") {
      const attempt = pendingMutation.current;
      if (attempt)
        await perform(attempt.path, attempt.body, attempt.message, true);
    } else if (recoveryRef.current) {
      const epoch = sessionEpoch.current;
      setBusy(true);
      try {
        await load(true);
      } finally {
        if (epoch === sessionEpoch.current) {
          mutationRunning.current = false;
          setBusy(false);
        }
      }
    }
  };
  const resolveSave = async (note) => {
    const attempt = pendingMutation.current;
    if (
      accessSuspended.current ||
      mutationRunning.current ||
      recoveryRef.current?.kind !== "unknown" ||
      !attempt
    )
      return;
    const epoch = sessionEpoch.current;
    mutationRunning.current = true;
    mutationEpoch.current++;
    setBusy(true);
    setError("");
    try {
      const scope = journalScope.current;
      const original = await journal(scope.owner, "check", {
        ...scope,
        key: attempt.key,
      });
      if (epoch !== sessionEpoch.current) return;
      if (original.generation !== scope.generation) {
        clearAccess("This workspace session has changed.", false);
        return;
      }
      if (
        original.attempt?.key !== attempt.key ||
        original.attempt.kind !== "unknown"
      ) {
        adoptJournal(original);
        await load(true);
        return;
      }
      const result = await request("save-recovery/resolve", {
        request_key: attempt.key,
        path: attempt.path,
        body: attempt.body,
        note,
      });
      if (epoch !== sessionEpoch.current) return;
      const decision = result.data;
      if (
        decision?.request_key !== attempt.key ||
        !["stopped", "completed"].includes(decision?.outcome) ||
        (decision.outcome === "stopped" &&
          decision.cancellation?.path !== attempt.path) ||
        (decision.outcome === "completed" && !("result" in decision))
      )
        throw new Error(
          "Original save resolution could not be verified. Check it again.",
        );
      const record = await journal(
        scope.owner,
        decision.outcome === "stopped" ? "stopped" : "confirmed",
        {
          ...scope,
          key: attempt.key,
        },
      );
      if (epoch !== sessionEpoch.current) return;
      adoptJournal(record);
      setModal(null);
      await load(true);
    } catch (problem) {
      if (epoch === sessionEpoch.current && !problem.sessionChanged)
        setError(problem.message);
    } finally {
      if (epoch === sessionEpoch.current) {
        mutationRunning.current = false;
        setBusy(false);
      }
    }
  };
  const customer = (id) =>
    workspace?.customers.find((v) => v.id === id)?.name || "Unknown customer";
  const carrier = (id) =>
    workspace?.carriers.find((v) => v.id === id)?.name || "Unassigned";
  const all = workspace?.loads || [];
  const historyOwner = workspace
    ? journalOwner(
        workspace,
        identity.mode,
        identity.organization,
        identity.session,
      )
    : "";
  const capabilities = workspace?.access?.capabilities || [];
  const canOperate =
    capabilities.includes("operations.write") &&
    !saveRecovery &&
    !journalProblem;
  const canFinance =
    capabilities.includes("finance.write") && !saveRecovery && !journalProblem;
  const canRetry =
    capabilities.includes("integrations.retry") &&
    !saveRecovery &&
    !journalProblem;
  const current = all.find((v) => v.id === selected);
  const commandLoad = modal?.kind === "command" ? modal.load : null;
  const commandChanged =
    modal?.kind === "command" &&
    (!current ||
      current.id !== commandLoad.id ||
      current.version !== commandLoad.version);
  const active = all.filter(
    (v) => !["delivered", "cancelled"].includes(v.status),
  );
  const exceptions = all.filter(
    (v) =>
      v.exception ||
      v.route_attention ||
      v.tender_control?.release_pending ||
      v.tender_control?.open_incidents ||
      v.stop_execution?.has_overdue,
  );
  const visible = all.filter(
    (v) =>
      (filter === "all" ||
        (filter === "exceptions"
          ? v.exception ||
            v.route_attention ||
            v.tender_control?.release_pending ||
            v.tender_control?.open_incidents ||
            v.stop_execution?.has_overdue
          : v.status === filter)) &&
      `${v.reference} ${v.origin.city} ${v.destination.city} ${customer(v.customer_id)} ${carrier(v.carrier_id)}`
        .toLowerCase()
        .includes(search.toLowerCase()),
  );
  const booked = all.filter((v) => v.status !== "cancelled");
  // Margin share is only meaningful within one currency; no FX is applied.
  const singleCurrency = currencySet(booked).size <= 1;
  const margin = booked.reduce((a, v) => a + sellTotal(v) - buyTotal(v), 0);
  const revenue = booked.reduce((a, v) => a + sellTotal(v), 0);
  const chooseAction = (item, action) => {
    setPodChoice("");
    setSelected(item.id);
    setError("");
    setModal({ kind: "command", action, load: item });
  };
  const exportLoads = () => {
    try {
      const csv = loadBoardCsv(
        visible.map((v) => [
          v.reference,
          customer(v.customer_id),
          `${v.origin.city}, ${v.origin.state}`,
          `${v.destination.city}, ${v.destination.state}`,
          v.status,
          carrier(v.carrier_id),
          buyTotal(v),
          sellTotal(v),
          sellTotal(v) - buyTotal(v),
          v.buy_cents,
          buyTotal(v) - v.buy_cents,
          v.sell_cents,
          sellTotal(v) - v.sell_cents,
          v.currency,
        ]),
      );
      const url = URL.createObjectURL(
        new Blob([csv], {
          type: "text/csv;charset=utf-8",
        }),
      );
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = "stateset-freight-loads.csv";
      anchor.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      setError(
        e.message ||
          "Shipments could not be exported. Refresh the workspace and try again.",
      );
    }
  };
  // Amount parsing throws before any request; show it like an API error.
  const formSubmit = (event) =>
    submitForm(event).catch((problem) => setError(problem.message));
  const submitForm = async (event) => {
    event.preventDefault();
    if (recoveryRef.current || mutationRunning.current) return;
    const values = Object.fromEntries(new FormData(event.currentTarget));
    if (modal.kind === "load" || modal.kind === "import") {
      const plan = {
        intermediate_stops: JSON.parse(values.intermediate_stops || "[]"),
        reference: values.reference,
        customer_id: values.customer_id,
        commodity: values.commodity,
        mode: values.mode,
        equipment: values.equipment,
        weight_lbs: Number(values.weight_lbs),
        miles: Number(values.miles),
        currency: currencyCode(values.currency),
        buy_cents: parseMoney(values.buy, values.currency, { min: 0 }),
        sell_cents: parseMoney(values.sell, values.currency, { min: 0 }),
        origin: {
          facility: values.origin_facility,
          city: values.origin_city,
          state: values.origin_state.toUpperCase(),
          date: values.pickup,
          address_lines: [
            values.origin_address,
            values.origin_address_2,
          ].filter(Boolean),
          postal_code: values.origin_postal,
          country: values.origin_country.toUpperCase(),
          contact: values.origin_contact,
          phone: values.origin_phone,
          instructions: values.origin_instructions,
        },
        destination: {
          facility: values.destination_facility,
          city: values.destination_city,
          state: values.destination_state.toUpperCase(),
          date: values.delivery,
          address_lines: [
            values.destination_address,
            values.destination_address_2,
          ].filter(Boolean),
          postal_code: values.destination_postal,
          country: values.destination_country.toUpperCase(),
          contact: values.destination_contact,
          phone: values.destination_phone,
          instructions: values.destination_instructions,
        },
      };
      await perform(
        modal.kind === "import"
          ? "sync/import"
          : modal.record
            ? `loads/${modal.record.id}/plan`
            : "loads",
        modal.kind === "import"
          ? { order_id: modal.order.id, load: plan }
          : modal.record
            ? { expected_version: modal.record.version, plan }
            : plan,
        modal.kind === "import"
          ? modal.order.sourceSystem === "icommerce"
            ? "iCommerce order linked to a new freight load"
            : "Sync order linked to a new freight load"
          : modal.record
            ? "Load plan revised"
            : "Load created",
      );
    } else if (modal.kind === "command") {
      if (commandChanged) return;
      if (values.document_id === "") delete values.document_id;
      await perform(
        `loads/${commandLoad.id}/commands`,
        {
          ...values,
          action: modal.action,
          expected_version: commandLoad.version,
        },
        "Shipment updated",
      );
    } else if (modal.kind === "invoice") {
      // A carrier invoice is always in its shipment's currency.
      const currency = currencyCode(
        workspace.loads.find((v) => v.id === values.load_id)?.currency,
      );
      await perform(
        "invoices",
        {
          number: values.number,
          load_id: values.load_id,
          amount_cents: parseMoney(values.amount, currency, { min: 0 }),
          currency,
        },
        "Invoice received and audited",
      );
    } else if (modal.kind === "resolveInvoice") {
      await perform(
        `invoices/${modal.invoice.id}/resolve`,
        {
          amount_cents: parseMoney(values.amount, modal.invoice.currency, {
            min: 0,
          }),
          expected_amount_cents: modal.invoice.amount_cents,
          note: values.note,
        },
        "Invoice correction recorded",
      );
    } else if (["reviewInvoice", "approveInvoice"].includes(modal.kind)) {
      const entryReview = modal.kind === "reviewInvoice";
      await perform(
        `invoices/${modal.invoice.id}/${entryReview ? "entry-review" : "approve"}`,
        {
          expected_revision: modal.invoice.approval_control.revision,
          note: values.note,
          ...(entryReview
            ? { evidence_reference: values.evidence_reference }
            : {}),
        },
        entryReview
          ? "Invoice entry reviewed; independent approval required"
          : "Invoice approved",
      );
    } else {
      await perform(
        modal.record ? `${modal.kind}/${modal.record.id}` : modal.kind,
        modal.record ? { expected: modal.record, data: values } : values,
        modal.record ? "Record updated" : "Record added",
      );
    }
  };
  const loadTable = (items, compact = false) => (
    <div className="table-scroll">
      <table>
        <thead>
          <tr>
            <th>Load / customer</th>
            <th>Route</th>
            <th>Pickup / delivery</th>
            <th>Status</th>
            {!compact && <th>Carrier</th>}
            <th className="numeric">
              {role === "Shipper operations" ? "Freight cost" : "Sell / margin"}
            </th>
            <th aria-label="View load" />
          </tr>
        </thead>
        <tbody>
          {items.map((item) => (
            <tr
              key={item.id}
              onClick={() => {
                setSelected(item.id);
                setModal({ kind: "detail" });
              }}
            >
              <td>
                <button
                  className="text-link"
                  onClick={(e) => {
                    e.stopPropagation();
                    setSelected(item.id);
                    setModal({ kind: "detail" });
                  }}
                >
                  {item.reference}
                </button>
                <small>{customer(item.customer_id)}</small>
              </td>
              <td>
                <div className="route-cell">
                  <span className="route-dots">
                    <i />
                    <b />
                    <i />
                  </span>
                  <div>
                    <span>
                      {item.origin.city}, {item.origin.state}
                    </span>
                    <span>
                      {item.destination.city}, {item.destination.state}
                    </span>
                  </div>
                </div>
              </td>
              <td>
                <span>{date(item.origin.date)}</span>
                <small>{date(item.destination.date)}</small>
              </td>
              <td>
                <Badge value={item.status} />
                {!!(
                  item.exception ||
                  item.route_attention ||
                  item.stop_execution?.has_overdue ||
                  item.tender_control?.release_pending ||
                  item.tender_control?.open_incidents
                ) && (
                  <span className="exception-label">
                    <AlertTriangle size={11} />
                    Needs attention
                  </span>
                )}
              </td>
              {!compact && (
                <td>
                  <span>{carrier(item.carrier_id)}</span>
                  <small>
                    {item.mode} · {item.equipment}
                  </small>
                </td>
              )}
              <td className="numeric">
                <strong>
                  {cash(
                    role === "Shipper operations"
                      ? buyTotal(item)
                      : sellTotal(item),
                    item.currency,
                  )}
                </strong>
                <small
                  className={
                    sellTotal(item) >= buyTotal(item) ? "green" : "red"
                  }
                >
                  {role === "Shipper operations"
                    ? `${item.miles.toLocaleString()} mi`
                    : `${cash(sellTotal(item) - buyTotal(item), item.currency)} margin`}
                </small>
              </td>
              <td>
                <ChevronRight size={17} className="muted" />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {!items.length && (
        <Empty title="No loads found">
          Try another filter, or create your first load.
        </Empty>
      )}
    </div>
  );

  if (auth && identity.mode === "clerk")
    return (
      <main className="login">
        <section className="login-card">
          <h1>Freight access unavailable</h1>
          <a href="/portal">Open partner portal</a>
          <p>{error || "Choose an approved organization or sign in again."}</p>
          <button
            className="primary"
            disabled={busy || signOutFailed}
            onClick={() => {
              accessSuspended.current = false;
              load();
            }}
          >
            Retry access
          </button>
          <button className="secondary" disabled={busy} onClick={signOut}>
            {signOutFailed ? "Retry sign out" : "Sign out"}
          </button>
        </section>
      </main>
    );
  if (auth)
    return (
      <>
        <Head>
          <title>Sign in · StateSet Freight</title>
        </Head>
        <main className="login">
          <div className="login-card">
            <div className="brand">
              <span className="brand-icon">
                <Box size={23} />
              </span>
              stateset<span className="brand-product">freight</span>
            </div>
            <h1>Your freight, connected.</h1>
            <p>
              Sign in with the workspace credential issued by your
              administrator.
            </p>
            <form
              onSubmit={async (e) => {
                e.preventDefault();
                if (busy || signOutFailed) return;
                const epoch = sessionEpoch.current;
                setBusy(true);
                try {
                  const response = await fetch("/api/session", {
                    method: "POST",
                    headers: { "Content-Type": "application/json" },
                    body: JSON.stringify({
                      token: new FormData(e.currentTarget).get("token"),
                    }),
                  });
                  const result = await response.json();
                  if (epoch !== sessionEpoch.current) return;
                  if (!response.ok) throw new Error(result.error?.message);
                  accessSuspended.current = false;
                  await load();
                } catch (e) {
                  if (epoch === sessionEpoch.current) setError(e.message);
                } finally {
                  if (epoch === sessionEpoch.current) setBusy(false);
                }
              }}
            >
              <Field
                title="Workspace credential"
                name="token"
                type="password"
                autoComplete="current-password"
                required
              />
              {error && (
                <p className="form-error" role="alert">
                  {error}
                </p>
              )}
              <button className="primary" disabled={busy || signOutFailed}>
                {signingOut
                  ? "Signing out…"
                  : busy
                    ? "Signing in…"
                    : "Open workspace"}
                <ArrowRight size={16} />
              </button>
            </form>
            {signOutFailed && (
              <button className="secondary" disabled={busy} onClick={signOut}>
                Retry sign out
              </button>
            )}
            <p>
              <a href="/portal">Carrier or customer access</a>
            </p>
          </div>
        </main>
      </>
    );
  return (
    <>
      <Head>
        <title>{`StateSet Freight · ${tabs.find((v) => v[0] === page)?.[1]}`}</title>
        <meta
          name="description"
          content="Connected freight operations for shippers, brokers, and carriers."
        />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
      </Head>
      <div className="app-shell">
        <aside className={`sidebar ${mobile ? "open" : ""}`}>
          <div className="brand">
            <span className="brand-icon">
              <Box size={23} />
            </span>
            stateset<span className="brand-product">freight</span>
          </div>
          <div className="workspace-switch">
            <div className="workspace-avatar">S</div>
            <div>
              <strong>StateSet operations</strong>
              <small>{workspace?.tenant_id || "Connecting…"}</small>
            </div>
            <ChevronDown size={15} />
          </div>
          <div className="nav-heading">WORKSPACE</div>
          <nav>
            {tabs.map(([key, text, Icon]) => (
              <button
                key={key}
                className={page === key ? "active" : ""}
                onClick={() => navigate(key)}
              >
                <Icon size={18} />
                {text}
                {key === "loads" && (
                  <span className="nav-count">{all.length}</span>
                )}
                {key === "tracking" && exceptions.length > 0 && (
                  <span className="nav-alert">{exceptions.length}</span>
                )}
              </button>
            ))}
            <a href="/catalog" className="catalog-nav">
              <Search size={18} />
              Shipment catalog
            </a>
          </nav>
          <div className="sidebar-bottom">
            <div className="connection">
              <span className={error ? "dot amber" : "dot"} />
              {workspace?.mode === "demo"
                ? "Demo workspace"
                : "Freight operations"}
              <small>
                {workspace?.storage === "ephemeral"
                  ? "Data resets on API restart"
                  : "PostgreSQL persistence"}
              </small>
            </div>
            <button
              className="profile"
              onClick={signOut}
              disabled={workspace?.mode === "demo" && identity.mode !== "clerk"}
            >
              <div className="profile-avatar">OP</div>
              <span>
                <strong>
                  {identity.mode === "clerk"
                    ? "Staff account"
                    : "Operations team"}
                </strong>
                <small>
                  {workspace?.mode === "demo"
                    ? "Synthetic demo data"
                    : "Sign out"}
                </small>
              </span>
              <LogOut size={16} />
            </button>
          </div>
        </aside>
        <div className="main">
          <header className="topbar">
            <button
              className="icon-button mobile-menu"
              aria-label="Toggle navigation"
              onClick={() => setMobile(!mobile)}
            >
              <Menu size={20} />
            </button>
            <div className="breadcrumb">
              Workspace
              <ChevronRight size={14} />
              <strong>Freight</strong>
            </div>
            <div className="topbar-right">
              {workspace?.access && (
                <span
                  className="access-label"
                  aria-label="Workspace access role"
                >
                  {label(workspace.access.role)} access
                </span>
              )}
              <span className="today">
                {new Date().toLocaleDateString("en-US", {
                  weekday: "short",
                  month: "short",
                  day: "numeric",
                  year: "numeric",
                })}
              </span>
              <span className="top-separator" />
              <button
                className="icon-button"
                aria-label="View shipment exceptions"
                onClick={() => {
                  navigate("tracking");
                  setFilter("exceptions");
                }}
              >
                <Bell size={18} />
                {exceptions.length > 0 && <i className="notification-dot" />}
              </button>
              <div className="user-avatar">OP</div>
            </div>
          </header>
          <main className="content">
            <div className="page-heading">
              <div>
                <div className="eyebrow">STATESET FREIGHT</div>
                <h1>{tabs.find((v) => v[0] === page)?.[1]}</h1>
                <p>
                  {
                    {
                      overview:
                        "Every shipment. Every mile. One connected workspace.",
                      operations:
                        "Prioritize source-backed issues, own the next step, and verify recovery.",
                      loads:
                        "Plan, tender, and move your freight with confidence.",
                      quotes:
                        "Retain carrier offers, confirm customer rates, and book accepted freight.",
                      tracking:
                        "Know where your freight is, and what needs your attention.",
                      dispatch:
                        "Keep your drivers, equipment, and shipments moving together.",
                      carriers: "Build a reliable network for every lane.",
                      customers: "Your shipping relationships, in one place.",
                      finance:
                        "Match carrier invoices, resolve variance, and prepare settlements.",
                      integrations:
                        "Connect the systems behind every shipment.",
                      activity:
                        "A complete history of your freight operations.",
                    }[page]
                  }
                </p>
              </div>
              <div className="heading-actions">
                <select
                  aria-label="Operations perspective"
                  className="role-select"
                  value={role}
                  onChange={(e) => setRole(e.target.value)}
                >
                  {[
                    "All operations",
                    "Shipper operations",
                    "Brokerage operations",
                    "Carrier operations",
                  ].map((v) => (
                    <option key={v}>{v}</option>
                  ))}
                </select>
                {page !== "quotes" &&
                  page !== "operations" &&
                  (page !== "finance" ||
                    financeSection === "carrier_audit") && (
                    <button
                      className="primary"
                      disabled={
                        !workspace ||
                        loading ||
                        (page === "finance" ? !canFinance : !canOperate)
                      }
                      onClick={() => {
                        setError("");
                        setModal({
                          kind:
                            page === "carriers"
                              ? "carriers"
                              : page === "customers"
                                ? "customers"
                                : page === "finance"
                                  ? "invoice"
                                  : "load",
                        });
                      }}
                    >
                      <Plus size={16} />
                      {page === "carriers"
                        ? "Add carrier"
                        : page === "customers"
                          ? "Add customer"
                          : page === "finance"
                            ? "Add invoice"
                            : "Create load"}
                    </button>
                  )}
              </div>
            </div>
            {workspace?.mode === "demo" && (
              <div className="demo-banner">
                <Circle size={12} />
                <span>
                  Demo workspace · sample freight data
                  {workspace.storage === "ephemeral"
                    ? " · changes reset when the API restarts"
                    : ""}
                </span>
                <span className="demo-pill">SANDBOX</span>
              </div>
            )}
            {journalProblem && !modal && (
              <RecoveryStorage
                problem={journalProblem}
                busy={busy}
                retry={() => load(true)}
              />
            )}
            {error && !modal && !saveRecovery && (
              <div className="error-banner" role="alert">
                <AlertTriangle size={17} />
                <span>{error}</span>
                <button onClick={() => load()} className="text-link">
                  Retry
                </button>
              </div>
            )}
            {saveRecovery && !modal && (
              <SaveRecovery
                outcome={saveRecovery}
                detail={error}
                busy={busy}
                retry={recoverSave}
                resolve={resolveSave}
              />
            )}
            {loading && !workspace ? (
              <div className="loading">
                <Loader2 size={24} className="spin" />
                <p>Connecting your freight workspace…</p>
              </div>
            ) : (
              workspace && (
                <>
                  {page === "operations" && (
                    <OperationsWorkspace
                      workspace={workspace}
                      onChanged={() => load(true)}
                      openLoad={(id) => {
                        setSelected(id);
                        setModal({ kind: "detail" });
                      }}
                      openIntegrations={() => navigate("integrations")}
                    />
                  )}
                  {page === "quotes" && (
                    <CarrierContracts
                      workspace={workspace}
                      onChanged={() => load(true)}
                    />
                  )}
                  {page === "quotes" && (
                    <QuoteWorkspace
                      workspace={workspace}
                      onChanged={() => load(true)}
                      openLoad={(id) => {
                        setSelected(id);
                        setModal({ kind: "detail" });
                      }}
                      initialQuoteId={quoteSelection}
                    />
                  )}
                  {page === "overview" && (
                    <>
                      <div className="metrics">
                        <Metric
                          title="Active loads"
                          value={active.length.toString().padStart(2, "0")}
                          detail={`${all.filter((v) => v.status === "in_transit").length} shipments in transit`}
                          Icon={Truck}
                        />
                        <Metric
                          title="Delivered loads"
                          value={all
                            .filter((v) => v.status === "delivered")
                            .length.toString()
                            .padStart(2, "0")}
                          detail="Confirmed shipment deliveries"
                          Icon={CheckCircle2}
                        />
                        <Metric
                          title={
                            role === "Shipper operations"
                              ? "Contracted freight"
                              : "Booked revenue"
                          }
                          value={formatTotals(
                            booked,
                            role === "Shipper operations"
                              ? buyTotal
                              : sellTotal,
                          )}
                          detail={
                            singleCurrency
                              ? `${currencyCode(booked[0]?.currency)} · non-cancelled loads`
                              : "Per currency, unconverted · non-cancelled loads"
                          }
                          Icon={DollarSign}
                        />
                        <Metric
                          title="Gross margin"
                          value={formatTotals(
                            booked,
                            (v) => sellTotal(v) - buyTotal(v),
                          )}
                          detail={
                            singleCurrency
                              ? `${revenue ? ((margin / revenue) * 100).toFixed(1) : "0.0"}% of booked revenue`
                              : "Per currency; no exchange rate applied"
                          }
                          Icon={ArrowUpRight}
                        />
                      </div>
                      <div className="overview-grid">
                        <section className="panel pipeline">
                          <div className="panel-heading">
                            <div>
                              <h2>Your freight at a glance</h2>
                              <p>Shipment lifecycle across your workspace</p>
                            </div>
                            <span className="live-label">
                              <span className="dot" />
                              Workspace data
                            </span>
                          </div>
                          <div className="pipeline-track">
                            {[
                              ["planned", "Planning", Box],
                              ["tendered", "Tendered", ExternalLink],
                              ["accepted", "Accepted", Check],
                              ["dispatched", "Dispatched", Truck],
                              ["in_transit", "In transit", MapPin],
                              ["delivered", "Delivered", FileCheck2],
                            ].map(([key, text, Icon]) => (
                              <button
                                key={key}
                                onClick={() => {
                                  navigate("loads");
                                  setFilter(key);
                                }}
                              >
                                <span className={`pipeline-icon ${key}`}>
                                  <Icon size={18} />
                                </span>
                                <strong>
                                  {all.filter((v) => v.status === key).length}
                                </strong>
                                <small>{text}</small>
                              </button>
                            ))}
                          </div>
                        </section>
                        <section className="panel attention">
                          <div className="panel-heading">
                            <div>
                              <h2>
                                <span className="attention-icon">
                                  <AlertTriangle size={16} />
                                </span>
                                Needs attention
                              </h2>
                              <p>Stay ahead of shipment exceptions</p>
                            </div>
                            <span className="count-bubble">
                              {exceptions.length}
                            </span>
                          </div>
                          {exceptions.length ? (
                            exceptions.slice(0, 2).map((v) => (
                              <button
                                className="attention-item"
                                key={v.id}
                                onClick={() => {
                                  setSelected(v.id);
                                  setModal({ kind: "detail" });
                                }}
                              >
                                <span>
                                  <strong>
                                    {v.reference}
                                    <small>
                                      {v.origin.city} → {v.destination.city}
                                    </small>
                                  </strong>
                                  <p>
                                    {v.exception ||
                                      v.route_attention ||
                                      (v.tender_control?.open_incidents
                                        ? "Carrier conflict needs resolution"
                                        : v.tender_control?.release_pending
                                          ? "Carrier release pending"
                                          : "Appointment window elapsed; check arrival")}
                                  </p>
                                </span>
                                <ChevronRight size={18} />
                              </button>
                            ))
                          ) : (
                            <div className="all-clear">
                              <CheckCircle2 size={20} />
                              No open shipment exceptions
                            </div>
                          )}
                        </section>
                      </div>
                      <section className="panel">
                        <div className="panel-heading">
                          <div>
                            <h2>
                              Recent loads{" "}
                              <span className="subtle-count">{all.length}</span>
                            </h2>
                            <p>
                              Your latest shipments, from planning to delivery
                            </p>
                          </div>
                          <button
                            className="text-link"
                            onClick={() => navigate("loads")}
                          >
                            View load board
                            <ArrowRight size={14} />
                          </button>
                        </div>
                        {loadTable(
                          [...all]
                            .sort(
                              (a, b) =>
                                new Date(b.created_at) - new Date(a.created_at),
                            )
                            .slice(0, 6),
                          true,
                        )}
                      </section>
                      <div className="overview-footer">
                        <span>
                          <ShieldCheck size={15} />
                          One shipment record across shipper, broker, and
                          carrier operations
                        </span>
                        <button
                          className="text-link"
                          onClick={() => navigate("integrations")}
                        >
                          Manage connections
                          <ArrowRight size={14} />
                        </button>
                      </div>
                    </>
                  )}
                  {(page === "loads" || page === "tracking") && (
                    <section className="panel">
                      <div className="board-tabs">
                        {[
                          ["all", "All loads"],
                          ["planned", "Planning"],
                          ["tendered", "Tendered"],
                          ["in_transit", "In transit"],
                          ["delivered", "Delivered"],
                          ["exceptions", "Exceptions"],
                        ].map(([key, text]) => (
                          <button
                            key={key}
                            className={filter === key ? "selected" : ""}
                            onClick={() => setFilter(key)}
                          >
                            {text}
                            <span>
                              {key === "all"
                                ? all.length
                                : key === "exceptions"
                                  ? exceptions.length
                                  : all.filter((v) => v.status === key).length}
                            </span>
                          </button>
                        ))}
                      </div>
                      <div className="table-toolbar">
                        <div className="search">
                          <Search size={16} />
                          <input
                            aria-label="Search loads"
                            placeholder="Search loads, customers, or lanes…"
                            value={search}
                            onChange={(e) => setSearch(e.target.value)}
                          />
                        </div>
                        <div className="toolbar-actions">
                          <button
                            className="secondary"
                            onClick={() => load(true)}
                            aria-label="Refresh load board"
                          >
                            <RefreshCw size={15} />
                          </button>
                          <button className="secondary" onClick={exportLoads}>
                            <Download size={15} />
                            Export
                          </button>
                        </div>
                      </div>
                      {loadTable(visible)}
                      <div className="table-footer">
                        {visible.length} of {all.length} loads
                        <span>
                          Shipment currency · all rates include configured
                          charges
                        </span>
                      </div>
                    </section>
                  )}
                  {page === "dispatch" && (
                    <>
                      <div className="section-title">
                        <h2>Dispatch board</h2>
                        <span>{active.length} open loads</span>
                      </div>
                      <div className="dispatch-grid">
                        {active
                          .filter((v) =>
                            ["accepted", "dispatched", "in_transit"].includes(
                              v.status,
                            ),
                          )
                          .map((v) => (
                            <section className="panel dispatch-card" key={v.id}>
                              <div className="card-top">
                                <strong>{v.reference}</strong>
                                <Badge value={v.status} />
                              </div>
                              <h3>
                                {v.origin.city}
                                <ArrowRight size={16} />
                                {v.destination.city}
                              </h3>
                              <p>
                                {carrier(v.carrier_id)} · {v.equipment}
                              </p>
                              <div className="assignment">
                                <Users size={16} />
                                <span>
                                  {workspace.drivers.find(
                                    (d) => d.id === v.driver_id,
                                  )?.name || "Driver unassigned"}
                                </span>
                              </div>
                              <div className="assignment">
                                <Truck size={16} />
                                <span>
                                  {workspace.equipment.find(
                                    (d) => d.id === v.equipment_id,
                                  )?.unit || "Equipment unassigned"}
                                </span>
                              </div>
                              <button
                                className="secondary full"
                                disabled={
                                  !canOperate ||
                                  v.tender_control?.release_pending ||
                                  v.tender_control?.open_incidents
                                }
                                onClick={() =>
                                  chooseAction(
                                    v,
                                    v.status === "accepted"
                                      ? "dispatch"
                                      : v.status === "dispatched"
                                        ? "pickup"
                                        : "deliver",
                                  )
                                }
                              >
                                {v.status === "accepted"
                                  ? "Assign & dispatch"
                                  : v.status === "dispatched"
                                    ? "Confirm pickup"
                                    : "Confirm delivery"}
                                <ArrowRight size={15} />
                              </button>
                            </section>
                          ))}
                      </div>
                      <div className="split-grid">
                        <ResourceList
                          writable={canOperate}
                          title="Drivers"
                          items={workspace.drivers}
                          add={() => setModal({ kind: "drivers" })}
                          edit={(record) =>
                            setModal({ kind: "drivers", record })
                          }
                          render={(v) => (
                            <>
                              <strong>{v.name}</strong>
                              <small>
                                {v.phone} · license through{" "}
                                {date(v.license_expires)}
                              </small>
                              <Badge
                                value={
                                  active.some(
                                    (l) =>
                                      ["dispatched", "in_transit"].includes(
                                        l.status,
                                      ) && l.driver_id === v.id,
                                  )
                                    ? "assigned"
                                    : "available"
                                }
                              />
                            </>
                          )}
                        />
                        <ResourceList
                          writable={canOperate}
                          title="Equipment"
                          items={workspace.equipment}
                          add={() => setModal({ kind: "equipment" })}
                          edit={(record) =>
                            setModal({ kind: "equipment", record })
                          }
                          render={(v) => (
                            <>
                              <strong>
                                {v.unit} · {v.kind}
                              </strong>
                              <small>{v.plate}</small>
                              <Badge
                                value={
                                  active.some(
                                    (l) =>
                                      ["dispatched", "in_transit"].includes(
                                        l.status,
                                      ) && l.equipment_id === v.id,
                                  )
                                    ? "assigned"
                                    : "available"
                                }
                              />
                            </>
                          )}
                        />
                      </div>
                    </>
                  )}
                  {page === "carriers" && (
                    <div className="resource-grid">
                      {workspace.carriers.map((v) => (
                        <section className="panel carrier-card" key={v.id}>
                          <div className="card-top">
                            <span className="carrier-logo">
                              <Truck size={23} />
                            </span>
                            <Badge
                              value={
                                v.insurance_expires >=
                                new Date().toISOString().slice(0, 10)
                                  ? "qualified"
                                  : "expired"
                              }
                            />
                          </div>
                          <h2>{v.name}</h2>
                          <p>
                            {v.scac} · {v.mc_number}
                          </p>
                          <dl>
                            <div>
                              <dt>Equipment</dt>
                              <dd>{v.equipment}</dd>
                            </div>
                            <div>
                              <dt>Insurance through</dt>
                              <dd>{date(v.insurance_expires)}</dd>
                            </div>
                            <div>
                              <dt>EDI partner</dt>
                              <dd>{v.partner_id}</dd>
                            </div>
                            <div>
                              <dt>Assigned loads</dt>
                              <dd>
                                {
                                  all.filter((l) => l.carrier_id === v.id)
                                    .length
                                }
                              </dd>
                            </div>
                          </dl>
                          <button
                            className="secondary full"
                            onClick={() => {
                              navigate("loads");
                              setSearch(v.name);
                            }}
                          >
                            View carrier loads
                            <ArrowRight size={14} />
                          </button>
                          <button
                            className="secondary full edit-record"
                            disabled={!canOperate}
                            onClick={() =>
                              setModal({ kind: "carriers", record: v })
                            }
                          >
                            Edit carrier
                          </button>
                        </section>
                      ))}
                      {!workspace.carriers.length && (
                        <Empty title="Build your carrier network">
                          Add your first carrier to start tendering.
                        </Empty>
                      )}
                    </div>
                  )}
                  {page === "customers" && (
                    <div className="resource-grid">
                      {workspace.customers.map((v) => (
                        <section className="panel carrier-card" key={v.id}>
                          <span className="carrier-logo">
                            <Building2 size={23} />
                          </span>
                          <h2>{v.name}</h2>
                          <p>{v.contact || "No contact added"}</p>
                          <dl>
                            <div>
                              <dt>Total loads</dt>
                              <dd>
                                {
                                  all.filter((l) => l.customer_id === v.id)
                                    .length
                                }
                              </dd>
                            </div>
                            <div>
                              <dt>Booked revenue</dt>
                              <dd>
                                {formatTotals(
                                  booked.filter((l) => l.customer_id === v.id),
                                  sellTotal,
                                )}
                              </dd>
                            </div>
                          </dl>
                          <button
                            className="secondary full"
                            onClick={() => {
                              navigate("loads");
                              setSearch(v.name);
                            }}
                          >
                            View customer loads
                            <ArrowRight size={14} />
                          </button>
                          <button
                            className="secondary full edit-record"
                            disabled={!canOperate}
                            onClick={() =>
                              setModal({ kind: "customers", record: v })
                            }
                          >
                            Edit customer
                          </button>
                        </section>
                      ))}
                    </div>
                  )}
                  {page === "finance" && (
                    <FinanceWorkspace
                      workspace={workspace}
                      section={financeSection}
                      onSection={setFinanceSection}
                      onChanged={() => load(true)}
                      openLoad={(id) => {
                        setSelected(id);
                        setModal({ kind: "detail" });
                      }}
                    >
                      {(workspace.charges || []).some(
                        (v) => v.status === "proposed",
                      ) && (
                        <section
                          className="panel charge-review-queue"
                          aria-label="Charges awaiting finance review"
                        >
                          <div className="panel-heading">
                            <div>
                              <h2>Charges awaiting review</h2>
                              <p>
                                Verify each counterparty agreement before
                                financial posting.
                              </p>
                            </div>
                          </div>
                          {workspace.charges
                            .filter((v) => v.status === "proposed")
                            .map((charge) => (
                              <button
                                key={charge.id}
                                className="charge-queue-item"
                                onClick={() => {
                                  setSelected(charge.load_id);
                                  setModal({ kind: "detail" });
                                }}
                              >
                                <span>
                                  <strong>
                                    {
                                      all.find((v) => v.id === charge.load_id)
                                        ?.reference
                                    }{" "}
                                    ·{" "}
                                    {charge.side === "buy"
                                      ? "Carrier cost"
                                      : "Customer charge"}
                                  </strong>
                                  <small>{charge.description}</small>
                                </span>
                                <strong>
                                  {cash(charge.amount_cents, charge.currency)}
                                </strong>
                                <ChevronRight size={16} />
                              </button>
                            ))}
                        </section>
                      )}
                      {((workspace.carrier_documents || []).some(
                        (v) => v.status === "pending",
                      ) ||
                        (workspace.carrier_adjustments || []).some(
                          (v) => v.status === "draft",
                        )) && (
                        <section
                          className="panel carrier-review-queue"
                          aria-label="Carrier documents awaiting finance"
                        >
                          <div className="panel-heading">
                            <h2>
                              Carrier invoice classification and adjustments
                            </h2>
                          </div>
                          {[
                            ...(workspace.carrier_documents || []).filter(
                              (v) => v.status === "pending",
                            ),
                            ...(workspace.carrier_adjustments || []).filter(
                              (v) => v.status === "draft",
                            ),
                          ].map((item) => (
                            <button
                              className="charge-queue-item"
                              key={item.id}
                              onClick={() => {
                                setSelected(item.load_id);
                                setModal({ kind: "detail" });
                              }}
                            >
                              <span>
                                <strong>
                                  {
                                    all.find((v) => v.id === item.load_id)
                                      ?.reference
                                  }{" "}
                                  · {item.number}
                                </strong>
                                <small>
                                  {item.kind
                                    ? "Adjustment draft requires approval"
                                    : "Additional 210 requires finance classification"}
                                </small>
                              </span>
                              <ChevronRight size={16} />
                            </button>
                          ))}
                        </section>
                      )}
                      <div className="metrics three">
                        <Metric
                          title="Awaiting approval"
                          value={formatTotals(
                            workspace.invoices.filter(
                              (v) => v.status === "matched",
                            ),
                            (v) => v.amount_cents,
                          )}
                          detail="Matched to base rate and approved carrier charges"
                          Icon={FileCheck2}
                        />
                        <Metric
                          title="Invoice variance"
                          value={formatTotals(workspace.invoices, (v) =>
                            Math.abs(v.variance_cents),
                          )}
                          detail="Review charge agreements or an actual revised invoice"
                          Icon={AlertTriangle}
                        />
                        <Metric
                          title="Prepared settlements"
                          value={summaryTotals(
                            workspace.finance,
                            "prepared_payable_cents",
                          )}
                          detail="Outstanding prepared carrier obligations"
                          Icon={Wallet}
                        />
                      </div>
                      <section className="panel">
                        <div className="panel-heading">
                          <div>
                            <h2>Carrier invoices</h2>
                            <p>
                              Rate matching and proof of delivery protect every
                              approval
                            </p>
                          </div>
                        </div>
                        <div className="table-scroll">
                          <table>
                            <thead>
                              <tr>
                                <th>Invoice</th>
                                <th>Shipment</th>
                                <th>Amount</th>
                                <th>Variance</th>
                                <th>Status</th>
                                <th>Action</th>
                              </tr>
                            </thead>
                            <tbody>
                              {workspace.invoices.map((v) => {
                                const shipment = all.find(
                                  (l) => l.id === v.load_id,
                                );
                                return (
                                  <tr key={v.id}>
                                    <td>
                                      <strong>{v.number}</strong>
                                      <small>
                                        {carrier(shipment?.carrier_id)}
                                      </small>
                                      <InvoiceApprovalEvidence
                                        invoice={v}
                                        owner={historyOwner}
                                        read={readHistory}
                                      />
                                    </td>
                                    <td>
                                      {shipment?.reference}
                                      <button
                                        className="text-link"
                                        onClick={() => {
                                          setSelected(v.load_id);
                                          setModal({ kind: "detail" });
                                        }}
                                      >
                                        Review shipment charges
                                      </button>
                                      <small>
                                        {shipment?.pod_reference
                                          ? "POD on file"
                                          : "Awaiting POD"}
                                      </small>
                                    </td>
                                    <td>{cash(v.amount_cents, v.currency)}</td>
                                    <td
                                      className={
                                        v.variance_cents ? "red" : "green"
                                      }
                                    >
                                      {cash(v.variance_cents, v.currency)}
                                    </td>
                                    <td>
                                      <Badge value={v.status} />
                                    </td>
                                    <td>
                                      {v.status === "matched" ? (
                                        <button
                                          disabled={
                                            busy ||
                                            !canFinance ||
                                            shipment?.status !== "delivered" ||
                                            !shipment?.pod_ready ||
                                            !!shipment?.commercial
                                              ?.pending_buy ||
                                            (v.approval_control?.required &&
                                              !v.approval_control.can_approve &&
                                              (v.approval_control.state ===
                                                "independent_approver_required" ||
                                                !v.approval_control
                                                  .can_review_entry))
                                          }
                                          className="secondary"
                                          onClick={() =>
                                            v.approval_control?.required
                                              ? setModal({
                                                  kind: v.approval_control
                                                    .can_approve
                                                    ? "approveInvoice"
                                                    : "reviewInvoice",
                                                  invoice: v,
                                                })
                                              : perform(
                                                  `invoices/${v.id}/approve`,
                                                  {},
                                                  "Invoice approved",
                                                )
                                          }
                                        >
                                          {v.approval_control?.required
                                            ? v.approval_control.can_approve
                                              ? "Approve"
                                              : v.approval_control.state ===
                                                  "independent_approver_required"
                                                ? "Independent approval required"
                                                : v.approval_control
                                                      .can_review_entry
                                                  ? "Review entry"
                                                  : "Named sign-in required"
                                            : "Approve"}
                                        </button>
                                      ) : v.status === "exception" ? (
                                        <button
                                          disabled={!canFinance}
                                          className="secondary"
                                          onClick={() =>
                                            setModal({
                                              kind: "resolveInvoice",
                                              invoice: v,
                                            })
                                          }
                                        >
                                          Record correction
                                        </button>
                                      ) : workspace.settlements.some(
                                          (s) => s.load_id === v.load_id,
                                        ) ? (
                                        <span className="green">
                                          {workspace.settlements.find(
                                            (s) => s.load_id === v.load_id,
                                          )?.status === "reconciled"
                                            ? "Carrier account reconciled"
                                            : workspace.settlements.find(
                                                  (s) =>
                                                    s.load_id === v.load_id,
                                                )?.status ===
                                                "partially_reconciled"
                                              ? "Partially reconciled"
                                              : "Settlement prepared"}
                                        </span>
                                      ) : (
                                        <button
                                          className="secondary"
                                          disabled={busy || !canFinance}
                                          onClick={() =>
                                            perform(
                                              `loads/${v.load_id}/settlement`,
                                              {},
                                              "Settlement prepared; no payment sent",
                                            )
                                          }
                                        >
                                          Prepare settlement
                                        </button>
                                      )}
                                      {v.status === "matched" &&
                                        shipment &&
                                        !shipment.pod_ready && (
                                          <button
                                            className="text-link"
                                            onClick={() => {
                                              setSelected(shipment.id);
                                              setModal({ kind: "detail" });
                                            }}
                                          >
                                            Review delivery evidence
                                          </button>
                                        )}
                                    </td>
                                  </tr>
                                );
                              })}
                            </tbody>
                          </table>
                          {!workspace.invoices.length && (
                            <Empty title="No carrier invoices yet">
                              Add an invoice or receive one through your EDI
                              connection.
                            </Empty>
                          )}
                        </div>
                      </section>
                    </FinanceWorkspace>
                  )}
                  {page === "integrations" && (
                    <>
                      <SourceQueue
                        workspace={workspace}
                        openLoad={(id) => {
                          setSelected(id);
                          setModal({ kind: "detail" });
                        }}
                      />
                      <div className="integration-grid">
                        {workspace.integrations.map((v, i) => (
                          <section
                            className="panel integration-card"
                            key={v.name}
                          >
                            <span className="integration-logo">
                              {i ? (
                                <RefreshCw size={25} />
                              ) : (
                                <Link2 size={25} />
                              )}
                            </span>
                            <Badge value={v.status} />
                            <h2>{v.name}</h2>
                            <p>{v.detail}</p>
                            <small>
                              {v.status === "configured"
                                ? "Connection configured on the freight API. Handoff results appear below."
                                : "Add tenant credentials to FREIGHT_INTEGRATIONS_JSON on the freight API."}
                            </small>
                            {(i === 1 || i === 2) && (
                              <button
                                className="secondary full"
                                disabled={
                                  v.status !== "configured" ||
                                  busy ||
                                  !canOperate
                                }
                                onClick={async () => {
                                  const epoch = sessionEpoch.current;
                                  setBusy(true);
                                  try {
                                    const result = await request("sync/orders");
                                    if (epoch !== sessionEpoch.current) return;
                                    setOrders(result);
                                    setOrderPage(1);
                                    setModal({ kind: "orders" });
                                  } catch (e) {
                                    if (epoch === sessionEpoch.current)
                                      setError(e.message);
                                  } finally {
                                    if (epoch === sessionEpoch.current)
                                      setBusy(false);
                                  }
                                }}
                              >
                                {i === 2
                                  ? "Import iCommerce order"
                                  : "Import sync order"}
                                <ArrowRight size={15} />
                              </button>
                            )}
                          </section>
                        ))}
                      </div>
                      <InboundDocuments
                        reviewLoad={(id) => {
                          setSelected(id);
                          setModal({ kind: "detail" });
                        }}
                        canRetry={canRetry}
                        items={workspace.inbox || []}
                        busy={busy}
                        inspect={(item) => setModal({ kind: "inbound", item })}
                        retry={(item) =>
                          perform(
                            `inbox/${item.id}/retry`,
                            {},
                            "Inbound document queued for processing",
                          )
                        }
                      />
                      <section className="panel">
                        <div className="panel-heading">
                          <div>
                            <h2>Integration handoffs</h2>
                            <p>
                              Durable requests, stable retry keys, and upstream
                              confirmations
                            </p>
                          </div>
                          <button
                            className="secondary"
                            onClick={() => load(true)}
                          >
                            <RefreshCw size={14} />
                            Refresh
                          </button>
                        </div>
                        {workspace.outbox.length ? (
                          <div className="handoffs">
                            {[...workspace.outbox].reverse().map((v) => (
                              <div className="handoff" key={v.id}>
                                <span className="handoff-icon">
                                  <ArrowUpRight size={18} />
                                </span>
                                <div>
                                  <strong>
                                    {
                                      all.find((l) => l.id === v.load_id)
                                        ?.reference
                                    }{" "}
                                    → {v.destination}
                                  </strong>
                                  <small>
                                    {v.path} · {v.attempts} attempts
                                  </small>
                                  {v.last_error && (
                                    <small
                                      className={
                                        v.status === "local_only" ? "" : "red"
                                      }
                                    >
                                      {v.last_error}
                                    </small>
                                  )}
                                </div>
                                <Badge value={v.status} />
                                {canOperate && (
                                  <CommerceHandoffReview
                                    key={`native-review-${journalScope.current?.owner || ""}-${v.id}`}
                                    job={v}
                                    load={all.find((l) => l.id === v.load_id)}
                                    disabled={busy}
                                    onQueue={perform}
                                  />
                                )}
                                {["held", "retry"].includes(v.status) && (
                                  <button
                                    className="secondary"
                                    disabled={busy || !canRetry}
                                    onClick={() =>
                                      perform(
                                        `outbox/${v.id}/retry`,
                                        {},
                                        "Handoff queued for retry",
                                      )
                                    }
                                  >
                                    Retry
                                  </button>
                                )}
                                <button
                                  className="icon-button"
                                  aria-label="Inspect handoff payload"
                                  onClick={() =>
                                    setModal({ kind: "payload", job: v })
                                  }
                                >
                                  <ExternalLink size={16} />
                                </button>
                              </div>
                            ))}
                          </div>
                        ) : (
                          <Empty title="No handoffs queued">
                            Tender a load, update a Sync shipment, or queue a
                            reviewed iCommerce shipment step.
                          </Empty>
                        )}
                      </section>
                    </>
                  )}
                  {page === "activity" && (
                    <section className="panel">
                      <RecordHistory
                        kind="events"
                        owner={historyOwner}
                        read={readHistory}
                        heading="Workspace history"
                      />
                    </section>
                  )}
                </>
              )
            )}
          </main>
          <footer className="footer">
            <span>StateSet Freight</span>
            <span>Connected operations, from tender to settlement.</span>
          </footer>
        </div>
      </div>
      {toast && (
        <div className="toast" role="status">
          <CheckCircle2 size={18} />
          {toast}
        </div>
      )}
      {modal && (
        <Modal
          title={
            modal.kind === "detail"
              ? current?.reference || "Load"
              : modal.kind === "command"
                ? `${label(modal.action)} · ${commandLoad.reference}`
                : modal.kind === "load"
                  ? modal.record
                    ? "Edit load plan"
                    : "Create a load"
                  : modal.kind === "import"
                    ? `Plan freight · ${modal.order.orderNumber}`
                    : modal.kind === "orders"
                      ? orders?.sourceSystem === "icommerce"
                        ? "Import from StateSet iCommerce"
                        : "Import from StateSet Sync"
                      : modal.kind === "inbound"
                        ? "Inbound EDI document"
                        : modal.kind === "payload"
                          ? "Integration handoff"
                          : modal.kind === "resolveInvoice"
                            ? "Record a corrected carrier invoice"
                            : modal.kind === "reviewInvoice"
                              ? "Review carrier invoice entry"
                              : modal.kind === "approveInvoice"
                                ? "Independently approve carrier invoice"
                                : `${modal.record ? "Edit" : "Add"} ${modal.kind === "equipment" ? "equipment" : modal.kind === "invoice" ? "invoice" : modal.kind.slice(0, -1)}`
          }
          subtitle={
            ["load", "import"].includes(modal.kind)
              ? "Define the lane, requirements, and contracted rates."
              : undefined
          }
          wide={[
            "load",
            "import",
            "detail",
            "orders",
            "payload",
            "inbound",
          ].includes(modal.kind)}
          close={() => {
            if (!busy) {
              setModal(null);
              setError("");
            }
          }}
        >
          {journalProblem && (
            <RecoveryStorage
              problem={journalProblem}
              busy={busy}
              retry={() => load(true)}
            />
          )}
          {saveRecovery && (
            <SaveRecovery
              outcome={saveRecovery}
              detail={error}
              busy={busy}
              retry={recoverSave}
              resolve={resolveSave}
            />
          )}
          {error && !saveRecovery && (
            <div className="form-error" role="alert">
              {error}
            </div>
          )}
          {modal.kind === "detail" && current ? (
            <div className="detail">
              <div className="detail-badges">
                <Badge value={current.status} />
                <span>
                  {current.mode} · {current.equipment} ·{" "}
                  {current.weight_lbs.toLocaleString()} lbs
                </span>
              </div>
              <div className="detail-route">
                <div>
                  <span className="stop-label">PICKUP</span>
                  <h3>
                    {current.origin.city}, {current.origin.state}
                  </h3>
                  <p>{current.origin.facility}</p>
                  <strong>{date(current.origin.date)}</strong>
                </div>
                <ArrowRight size={24} />
                <div>
                  <span className="stop-label">DELIVERY</span>
                  <h3>
                    {current.destination.city}, {current.destination.state}
                  </h3>
                  <p>{current.destination.facility}</p>
                  <strong>{date(current.destination.date)}</strong>
                </div>
              </div>
              {current.exception && (
                <div className="exception-box">
                  <AlertTriangle size={18} />
                  {current.exception}
                  <button
                    className="text-link"
                    disabled={busy || !canOperate}
                    onClick={() =>
                      perform(
                        `loads/${current.id}/commands`,
                        {
                          action: "resolve",
                          expected_version: current.version,
                        },
                        "Exception resolved",
                      )
                    }
                  >
                    Resolve
                  </button>
                </div>
              )}
              <dl className="detail-info">
                <div>
                  <dt>Customer</dt>
                  <dd>{customer(current.customer_id)}</dd>
                </div>
                <div>
                  <dt>Carrier</dt>
                  <dd>{carrier(current.carrier_id)}</dd>
                </div>
                <div>
                  <dt>Last reported location</dt>
                  <dd>{current.location}</dd>
                </div>
                <div>
                  <dt>Commodity</dt>
                  <dd>{current.commodity}</dd>
                </div>
                <div>
                  <dt>Agreed buy</dt>
                  <dd>{cash(buyTotal(current), current.currency)}</dd>
                </div>
                <div>
                  <dt>Sell / gross margin</dt>
                  <dd>
                    {cash(sellTotal(current), current.currency)} /{" "}
                    <span
                      className={
                        sellTotal(current) >= buyTotal(current)
                          ? "green"
                          : "red"
                      }
                    >
                      {cash(
                        sellTotal(current) - buyTotal(current),
                        current.currency,
                      )}
                    </span>
                  </dd>
                </div>
                <div>
                  <dt>Proof of delivery</dt>
                  <dd>
                    {current.pod_document_id
                      ? workspace.documents.find(
                          (v) => v.id === current.pod_document_id,
                        )?.filename
                      : current.pod_reference || "Awaiting evidence"}
                    {current.pod_reference && (
                      <small>
                        {current.route_attention ||
                          (current.tender_control?.open_incidents
                            ? "Carrier conflict must be resolved"
                            : current.pod_ready
                              ? "Reviewed delivery evidence"
                              : "Finance review required")}
                      </small>
                    )}
                  </dd>
                </div>
                <div>
                  <dt>Linked sync order</dt>
                  <dd>{current.order_number || "Not linked"}</dd>
                </div>
              </dl>
              {current.quote_binding && (
                <button
                  className="secondary"
                  onClick={() => {
                    setQuoteSelection(current.quote_binding.id);
                    setSelected(null);
                    setModal(null);
                    navigate("quotes");
                  }}
                >
                  View quote {current.quote_binding.reference}
                </button>
              )}
              <fieldset
                className="detail-actions permission-group"
                disabled={!canOperate}
              >
                {current.status === "planned" && !current.quote_binding && (
                  <button
                    className="secondary"
                    onClick={() => setModal({ kind: "load", record: current })}
                  >
                    Edit plan
                  </button>
                )}
                {(
                  {
                    planned: ["tender", "cancel"],
                    tendered: ["accept", "decline"],
                    accepted: ["dispatch"],
                    dispatched: ["pickup"],
                    in_transit: ["location", "deliver"],
                    delivered: ["attach_pod"],
                    cancelled: [],
                  }[current.status] || []
                ).map((action) => (
                  <button
                    key={action}
                    disabled={
                      ["dispatch", "tender"].includes(action) &&
                      (current.tender_control?.release_pending ||
                        current.tender_control?.open_incidents ||
                        current.source_control?.held)
                    }
                    className={
                      ["tender", "dispatch", "pickup", "deliver"].includes(
                        action,
                      )
                        ? "primary"
                        : "secondary"
                    }
                    onClick={() => chooseAction(current, action)}
                  >
                    {label(action)}
                    <ArrowRight size={14} />
                  </button>
                ))}
                {!["delivered", "cancelled"].includes(current.status) && (
                  <button
                    className="secondary"
                    onClick={() => chooseAction(current, "exception")}
                  >
                    <AlertTriangle size={14} />
                    Flag exception
                  </button>
                )}
              </fieldset>
              <SourceControl
                key={`source-${current.id}`}
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <CommerceTracking
                key={`commerce-${journalScope.current?.owner || ""}-${current.id}`}
                load={current}
                workspace={workspace}
                disabled={busy || !!recoveryRef.current}
                onQueue={perform}
                onOpenHandoffs={() => {
                  setModal(null);
                  navigate("integrations");
                }}
              />
              <CarrierPayables
                key={`payables-${current.id}`}
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <ShipmentCharges
                key={`charges-${current.id}`}
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <CancellationFees
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <ItineraryExecution
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <ExecutionTimeline timeline={current.execution_timeline} />
              <MovementCorrections
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <StopExecution
                key={current.id}
                readHistory={readHistory}
                historyOwner={historyOwner}
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <RateConfirmations
                key={`rates-${current.id}`}
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <TenderHistory
                load={current}
                workspace={workspace}
                onChanged={() => load(true)}
              />
              <ShipmentDocuments
                load={current}
                sharingContext={{
                  shares: workspace.document_shares || [],
                  tenders: workspace.tenders || [],
                  customerName: workspace.customers.find(
                    (c) => c.id === current.customer_id,
                  )?.name,
                  carrierName: workspace.carriers.find(
                    (c) => c.id === current.carrier_id,
                  )?.name,
                }}
                documents={(workspace.documents || []).filter(
                  (v) => v.load_id === current.id,
                )}
                capabilities={capabilities}
                onChanged={() => load(true)}
                locked={
                  workspace.invoices.some(
                    (v) => v.load_id === current.id && v.status === "approved",
                  ) ||
                  workspace.settlements.some((v) => v.load_id === current.id) ||
                  workspace.bills.some(
                    (v) =>
                      v.load_id === current.id &&
                      v.issued_at &&
                      v.status !== "void",
                  )
                }
              />
              <h3 className="detail-history">Shipment history</h3>
              <RecordHistory
                kind="events"
                loadId={current.id}
                owner={historyOwner}
                read={readHistory}
                heading="Shipment activity"
                limit={10}
              />
            </div>
          ) : modal.kind === "inbound" ? (
            <InboundDocumentDetail
              canRetry={canRetry}
              item={
                workspace.inbox.find((item) => item.id === modal.item.id) ||
                modal.item
              }
              busy={busy}
              retry={(item) =>
                perform(
                  `inbox/${item.id}/retry`,
                  {},
                  "Inbound document queued for processing",
                )
              }
            />
          ) : modal.kind === "payload" ? (
            <div className="payload">
              <Badge value={modal.job.status} />
              <p>
                {modal.job.destination === "stateset-icommerce"
                  ? "Native shipment handoff evidence. A local external review leaves the original PATCH outcome unknown."
                  : "Gateway acceptance confirms EDI generation and queuing. Carrier acceptance is a separate 990 response."}
              </p>
              <pre>{JSON.stringify(modal.job.payload, null, 2)}</pre>
              {modal.job.response && (
                <>
                  <h3>
                    {modal.job.status === "external_reviewed"
                      ? "Recorded external review"
                      : "Upstream response"}
                  </h3>
                  <pre>{modal.job.response}</pre>
                </>
              )}
            </div>
          ) : modal.kind === "orders" ? (
            <div className="order-list">
              <p>
                Select an order, then provide the freight plan. The original
                order remains owned by its configured commerce platform.
              </p>
              {orders?.items.map((v) => (
                <button
                  className="order-option"
                  key={v.id}
                  disabled={v.sourceSystem === "icommerce" && !v.importable}
                  onClick={() => setModal({ kind: "import", order: v })}
                >
                  <div>
                    <strong>{v.orderNumber}</strong>
                    <small>
                      {label(v.status)} ·{" "}
                      {v.items?.length ?? v.lineItems?.length ?? 0} items
                    </small>
                  </div>
                  <ArrowRight size={17} />
                </button>
              ))}
              {!orders?.items.length && (
                <Empty title="No source orders on this page" />
              )}
              <div className="pagination">
                <button
                  className="secondary"
                  disabled={orderPage <= 1 || busy}
                  onClick={async () => {
                    const epoch = sessionEpoch.current;
                    try {
                      const result = await request(
                        `sync/orders?page=${orderPage - 1}`,
                      );
                      if (epoch !== sessionEpoch.current) return;
                      setOrders(result);
                      setOrderPage(orderPage - 1);
                    } catch (e) {
                      if (epoch === sessionEpoch.current) setError(e.message);
                    }
                  }}
                >
                  Previous
                </button>
                <span>Page {orderPage}</span>
                <button
                  className="secondary"
                  disabled={!orders?.pagination?.hasNext || busy}
                  onClick={async () => {
                    const epoch = sessionEpoch.current;
                    try {
                      const result = await request(
                        `sync/orders?page=${orderPage + 1}`,
                      );
                      if (epoch !== sessionEpoch.current) return;
                      setOrders(result);
                      setOrderPage(orderPage + 1);
                    } catch (e) {
                      if (epoch === sessionEpoch.current) setError(e.message);
                    }
                  }}
                >
                  Next
                </button>
              </div>
            </div>
          ) : (
            <RecordDefaults.Provider value={fieldValues(modal.record)}>
              <form className="modal-form" onSubmit={formSubmit}>
                <fieldset
                  className="permission-group"
                  disabled={!!saveRecovery}
                >
                  {["load", "import"].includes(modal.kind) && (
                    <>
                      <div className="form-grid">
                        <Field
                          title="Load reference"
                          name="reference"
                          defaultValue={
                            modal.order
                              ? `FR-${modal.order.orderNumber.replace(/[^a-zA-Z0-9_-]/g, "")}`
                              : ""
                          }
                          placeholder="FR-1054"
                          required
                          maxLength={30}
                          pattern="[A-Za-z0-9_-]+"
                        />
                        <Field title="Customer" name="customer_id" required>
                          <option value="">Select customer</option>
                          {workspace.customers.map((v) => (
                            <option key={v.id} value={v.id}>
                              {v.name}
                            </option>
                          ))}
                        </Field>
                        <Field title="Mode" name="mode">
                          <option>FTL</option>
                          <option>LTL</option>
                        </Field>
                        <Field title="Equipment" name="equipment">
                          {["Dry van", "Reefer", "Flatbed"].map((v) => (
                            <option key={v}>{v}</option>
                          ))}
                        </Field>
                        <Field
                          title="Commodity"
                          name="commodity"
                          placeholder="Consumer goods"
                          required
                        />
                        <Field
                          title="Weight (lbs)"
                          name="weight_lbs"
                          type="number"
                          min="1"
                          max="80000"
                          defaultValue="24000"
                          required
                        />
                      </div>
                      <div className="form-section">
                        <MapPin size={15} />
                        Pickup
                      </div>
                      <div className="form-grid">
                        <Field
                          title="Facility"
                          name="origin_facility"
                          required
                        />
                        <Field
                          title="Pickup date"
                          name="pickup"
                          type="date"
                          defaultValue={new Date().toISOString().slice(0, 10)}
                          required
                        />
                        <Field title="City" name="origin_city" required />
                        <Field
                          title="State"
                          name="origin_state"
                          maxLength={2}
                          minLength={2}
                          required
                        />
                        <Field
                          title="Pickup street address"
                          name="origin_address"
                          maxLength={55}
                          required
                        />
                        <Field
                          title="Pickup address line 2"
                          name="origin_address_2"
                          maxLength={55}
                        />
                        <Field
                          title="Pickup postal code"
                          name="origin_postal"
                          maxLength={15}
                          required
                        />
                        <Field
                          title="Pickup country"
                          name="origin_country"
                          minLength={2}
                          maxLength={2}
                          defaultValue="US"
                          required
                        />
                        <Field
                          title="Pickup contact"
                          name="origin_contact"
                          maxLength={200}
                        />
                        <Field
                          title="Pickup phone"
                          name="origin_phone"
                          maxLength={100}
                        />
                        <Field
                          title="Pickup instructions"
                          name="origin_instructions"
                          maxLength={1000}
                        />
                      </div>
                      <div className="form-section">
                        <MapPin size={15} />
                        Delivery
                      </div>
                      <div className="form-grid">
                        <Field
                          title="Facility"
                          name="destination_facility"
                          required
                        />
                        <Field
                          title="Delivery date"
                          name="delivery"
                          type="date"
                          defaultValue={new Date(Date.now() + 86400000)
                            .toISOString()
                            .slice(0, 10)}
                          required
                        />
                        <Field title="City" name="destination_city" required />
                        <Field
                          title="State"
                          name="destination_state"
                          maxLength={2}
                          minLength={2}
                          required
                        />
                        <Field
                          title="Delivery street address"
                          name="destination_address"
                          maxLength={55}
                          required
                        />
                        <Field
                          title="Delivery address line 2"
                          name="destination_address_2"
                          maxLength={55}
                        />
                        <Field
                          title="Delivery postal code"
                          name="destination_postal"
                          maxLength={15}
                          required
                        />
                        <Field
                          title="Delivery country"
                          name="destination_country"
                          minLength={2}
                          maxLength={2}
                          defaultValue="US"
                          required
                        />
                        <Field
                          title="Delivery contact"
                          name="destination_contact"
                          maxLength={200}
                        />
                        <Field
                          title="Delivery phone"
                          name="destination_phone"
                          maxLength={100}
                        />
                        <Field
                          title="Delivery instructions"
                          name="destination_instructions"
                          maxLength={1000}
                        />
                      </div>
                      <div className="form-section">
                        <DollarSign size={15} />
                        Rates & distance
                      </div>
                      <div className="form-grid three">
                        <Field
                          title="Rate currency"
                          name="currency"
                          maxLength={3}
                          pattern="[A-Za-z]{3}"
                          defaultValue={
                            modal.kind === "import"
                              ? orderCurrency(modal.order)
                              : "USD"
                          }
                          required
                        />
                        <Field
                          title="Buy rate"
                          name="buy"
                          inputMode="decimal"
                          required
                        />
                        <Field
                          title="Sell rate"
                          name="sell"
                          inputMode="decimal"
                          required
                        />
                        <Field
                          title="Distance (miles)"
                          name="miles"
                          type="number"
                          min="1"
                          max="10000"
                          required
                        />
                      </div>
                      <IntermediateStopsEditor
                        key={modal.record?.id || modal.order?.id || "new"}
                        initial={modal.record?.intermediate_stops || []}
                      />
                      {!workspace.customers.length && (
                        <p className="form-error">
                          Add a customer before creating a load.
                        </p>
                      )}
                    </>
                  )}
                  {modal.kind === "command" && (
                    <>
                      {commandChanged && !saveRecovery && (
                        <p className="form-error" role="alert">
                          This shipment changed while this action was open. Your
                          entries are kept. Close this action and review the
                          latest shipment before continuing.
                        </p>
                      )}
                      {modal.action === "tender" && (
                        <>
                          <p>
                            {commandLoad.intermediate_stops?.length
                              ? "Prepare the full route for a qualified carrier. Publish the carrier confirmation in the portal to obtain acceptance of every stop."
                              : "Prepare an X12 204 tender for a qualified carrier. Transmission requires a configured EDI connection."}
                          </p>
                          <Field title="Carrier" name="carrier_id" required>
                            <option value="">Select carrier</option>
                            {workspace.carriers
                              .filter(
                                (v) =>
                                  v.equipment === commandLoad.equipment &&
                                  (!commandLoad.quote_binding ||
                                    v.id ===
                                      commandLoad.quote_binding.carrier_id),
                              )
                              .map((v) => (
                                <option key={v.id} value={v.id}>
                                  {v.name} · {v.scac}
                                </option>
                              ))}
                          </Field>
                        </>
                      )}
                      {modal.action === "dispatch" && (
                        <>
                          <Field title="Driver" name="driver_id" required>
                            <option value="">Select driver</option>
                            {workspace.drivers.map((v) => (
                              <option key={v.id} value={v.id}>
                                {v.name}
                              </option>
                            ))}
                          </Field>
                          <Field title="Equipment" name="equipment_id" required>
                            <option value="">Select equipment</option>
                            {workspace.equipment
                              .filter((v) => v.kind === commandLoad.equipment)
                              .map((v) => (
                                <option key={v.id} value={v.id}>
                                  {v.unit} · {v.kind}
                                </option>
                              ))}
                          </Field>
                        </>
                      )}
                      {["deliver", "attach_pod"].includes(modal.action) && (
                        <>
                          <Field
                            title="Proof of delivery source"
                            name="document_id"
                            value={podChoice}
                            onChange={(event) =>
                              setPodChoice(event.target.value)
                            }
                          >
                            <option value="">
                              External document reference
                            </option>
                            {(workspace.documents || [])
                              .filter(
                                (v) =>
                                  v.load_id === commandLoad.id &&
                                  v.kind === "pod" &&
                                  v.reviews.at(-1)?.decision !== "rejected",
                              )
                              .map((document) => (
                                <option key={document.id} value={document.id}>
                                  {document.filename}
                                </option>
                              ))}
                          </Field>
                          {!podChoice && (
                            <Field
                              title="Proof of delivery reference"
                              name="pod_reference"
                              placeholder="Reference to the actual signed delivery evidence"
                              required
                              maxLength={200}
                            />
                          )}
                          <p>
                            Finance must review delivery evidence before
                            approving the carrier invoice.
                          </p>
                        </>
                      )}
                      {modal.action === "location" && (
                        <Field
                          title="Reported location"
                          name="location"
                          placeholder="City, state or facility"
                          required
                        />
                      )}
                      {modal.action === "exception" && (
                        <Field
                          title="Exception details"
                          name="note"
                          placeholder="Describe what needs attention"
                          required
                        />
                      )}
                      {["accept", "decline", "pickup", "cancel"].includes(
                        modal.action,
                      ) && (
                        <p>
                          Record{" "}
                          {modal.action === "accept"
                            ? "carrier acceptance"
                            : modal.action === "decline"
                              ? "carrier rejection"
                              : modal.action === "pickup"
                                ? "confirmed pickup"
                                : "load cancellation"}{" "}
                          for {commandLoad.reference}. This action is recorded
                          in the shipment history.
                        </p>
                      )}
                    </>
                  )}
                  {modal.kind === "carriers" && (
                    <div className="form-grid">
                      <Field title="Carrier name" name="name" required />
                      <Field
                        title="SCAC"
                        name="scac"
                        minLength={2}
                        maxLength={4}
                        pattern="[A-Z]+"
                        required
                      />
                      <Field title="MC number" name="mc_number" required />
                      <Field
                        title="Insurance expiration"
                        name="insurance_expires"
                        type="date"
                        required
                      />
                      <Field title="Equipment" name="equipment">
                        {["Dry van", "Reefer", "Flatbed"].map((v) => (
                          <option key={v}>{v}</option>
                        ))}
                      </Field>
                      <Field
                        title="EDI partner ID"
                        name="partner_id"
                        pattern="[A-Za-z0-9_-]+"
                        required
                      />
                    </div>
                  )}
                  {modal.kind === "customers" && (
                    <>
                      <Field title="Customer name" name="name" required />
                      <Field title="Contact email or phone" name="contact" />
                      <Field
                        title="Billing address"
                        name="billing_address"
                        maxLength={1000}
                      />
                    </>
                  )}
                  {modal.kind === "drivers" && (
                    <>
                      <Field title="Driver name" name="name" required />
                      <Field title="Phone" name="phone" required />
                      <Field
                        title="License expiration"
                        name="license_expires"
                        type="date"
                        required
                      />
                    </>
                  )}
                  {modal.kind === "equipment" && (
                    <>
                      <Field title="Unit number" name="unit" required />
                      <Field title="Type" name="kind">
                        {["Dry van", "Reefer", "Flatbed"].map((v) => (
                          <option key={v}>{v}</option>
                        ))}
                      </Field>
                      <Field title="License plate" name="plate" required />
                    </>
                  )}
                  {modal.kind === "invoice" && (
                    <>
                      <Field
                        title="Carrier invoice number"
                        name="number"
                        required
                      />
                      <Field title="Load" name="load_id" required>
                        <option value="">Select load</option>
                        {all
                          .filter(
                            (v) =>
                              v.carrier_id &&
                              v.status !== "cancelled" &&
                              !workspace.invoices.some(
                                (i) => i.load_id === v.id,
                              ),
                          )
                          .map((v) => (
                            <option key={v.id} value={v.id}>
                              {v.reference} · agreed{" "}
                              {cash(buyTotal(v), v.currency)}
                            </option>
                          ))}
                      </Field>
                      <Field
                        title="Invoice total (shipment currency)"
                        name="amount"
                        inputMode="decimal"
                        required
                      />
                    </>
                  )}
                  {modal.kind === "resolveInvoice" && (
                    <>
                      <p>
                        Enter the corrected amount from the carrier’s revised
                        invoice. The original amount and your explanation remain
                        in the audit history.
                      </p>
                      <Field
                        title={`Corrected invoice total (${currencyCode(modal.invoice.currency)})`}
                        name="amount"
                        inputMode="decimal"
                        defaultValue={moneyInput(
                          modal.invoice.amount_cents,
                          modal.invoice.currency,
                        )}
                        required
                      />
                      <Field
                        title="Correction evidence / note"
                        name="note"
                        placeholder="Carrier revised invoice reference"
                        required
                      />
                    </>
                  )}
                  {["reviewInvoice", "approveInvoice"].includes(modal.kind) && (
                    <>
                      <p>
                        Invoice {modal.invoice.number} ·{" "}
                        {cash(
                          modal.invoice.amount_cents,
                          modal.invoice.currency,
                        )}
                        .
                        {modal.kind === "reviewInvoice"
                          ? " Check the carrier’s actual invoice and agreed charges. A different named finance user must approve this revision."
                          : ` Entry reviewed by ${modal.invoice.approval_control.entry_reviewed_by}. Verify the invoice, agreed charges, and delivery evidence before approval.`}
                      </p>
                      {modal.kind === "reviewInvoice" && (
                        <Field
                          title="Carrier invoice evidence reference"
                          name="evidence_reference"
                          minLength={10}
                          maxLength={2000}
                          required
                        />
                      )}
                      <Field
                        title={
                          modal.kind === "reviewInvoice"
                            ? "Entry review findings"
                            : "Independent approval findings"
                        }
                        name="note"
                        minLength={10}
                        maxLength={2000}
                        required
                      />
                    </>
                  )}
                  <div className="form-footer">
                    <button
                      type="button"
                      className="secondary"
                      onClick={() => setModal(null)}
                      disabled={busy}
                    >
                      Cancel
                    </button>
                    <button
                      className="primary"
                      disabled={
                        busy ||
                        commandChanged ||
                        ([
                          "invoice",
                          "resolveInvoice",
                          "reviewInvoice",
                          "approveInvoice",
                        ].includes(modal.kind)
                          ? !canFinance
                          : !canOperate) ||
                        (["load", "import"].includes(modal.kind) &&
                          !workspace.customers.length)
                      }
                    >
                      {busy ? (
                        <>
                          <Loader2 size={15} className="spin" />
                          Saving…
                        </>
                      ) : (
                        <>
                          {modal.kind === "command"
                            ? `Confirm ${label(modal.action).toLowerCase()}`
                            : modal.kind === "load"
                              ? modal.record
                                ? "Save changes"
                                : "Create load"
                              : modal.kind === "import"
                                ? "Import & create load"
                                : "Save record"}
                          <ArrowRight size={15} />
                        </>
                      )}
                    </button>
                  </div>
                </fieldset>
              </form>
            </RecordDefaults.Provider>
          )}
        </Modal>
      )}
    </>
  );
}
function Metric({ title, value, detail, Icon }) {
  return (
    <section className="metric">
      <div className="metric-title">
        {title}
        <Icon size={17} />
      </div>
      <strong>{value}</strong>
      <p>{detail}</p>
    </section>
  );
}
function ResourceList({ title, items, add, edit, render, writable }) {
  return (
    <section className="panel">
      <div className="panel-heading">
        <h2>{title}</h2>
        <button className="text-link" onClick={add} disabled={!writable}>
          <Plus size={14} />
          Add {title === "Drivers" ? "driver" : "equipment"}
        </button>
      </div>
      <div className="resource-list">
        {items.map((v) => (
          <div key={v.id}>
            {render(v)}
            {edit && (
              <button
                className="text-link resource-edit"
                disabled={!writable}
                onClick={() => edit(v)}
              >
                Edit {title === "Drivers" ? "driver" : "equipment"}
              </button>
            )}
          </div>
        ))}
        {!items.length && <Empty title={`No ${title.toLowerCase()} added`} />}
      </div>
    </section>
  );
}
