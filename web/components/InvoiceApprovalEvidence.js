import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Modal } from "./Modal";

function RecordedTime({ value }) {
  return (
    <time dateTime={value}>
      {new Date(value).toLocaleString(undefined, { timeZoneName: "short" })}
    </time>
  );
}

const decisionFields = [
  "revision",
  "entry_review_id",
  "entry_reviewed_by",
  "entry_reviewed_at",
  "approval_event_id",
  "approved_by",
  "approved_at",
];
const detailFields = [
  "evidence_reference",
  "entry_review_note",
  "approval_note",
];
export function InvoiceApprovalEvidence({ invoice, owner, read }) {
  const summary = invoice.approval_control?.approval_evidence;
  const scope = JSON.stringify([
    owner,
    invoice.id,
    invoice.number,
    ...decisionFields.map((field) => summary?.[field]),
  ]);
  const live = useRef(scope),
    sequence = useRef(0);
  live.current = scope;
  const [view, setView] = useState(null);
  useEffect(() => {
    sequence.current++;
    setView(null);
  }, [scope]);
  useEffect(
    () => () => {
      sequence.current++;
    },
    [],
  );
  const current = view?.scope === scope ? view : null;
  const evidence = current?.evidence;
  function close() {
    sequence.current++;
    setView(null);
  }
  async function open() {
    const ticket = ++sequence.current;
    setView({ scope, loading: true });
    try {
      // A preceding compatible API ignores the opt-in header and still sends
      // complete evidence. Keep that already-loaded record usable on rollout.
      let result;
      if (detailFields.every((field) => typeof summary?.[field] === "string"))
        result = summary;
      else {
        const response = await read(
          `invoices/${encodeURIComponent(invoice.id)}/approval-evidence`,
          owner,
        );
        if (
          response.invoice_id !== invoice.id ||
          response.number !== invoice.number
        )
          throw new Error(
            "Approval record does not match this invoice. Refresh the workspace.",
          );
        result = response.approval_evidence;
      }
      if (live.current !== scope || sequence.current !== ticket) return;
      if (
        !result ||
        !decisionFields.every((field) => result[field] === summary?.[field]) ||
        !detailFields.every((field) => typeof result[field] === "string")
      )
        throw new Error(
          "Approval record changed or is unavailable. Refresh the workspace.",
        );
      setView({ scope, evidence: result, loading: false });
    } catch (error) {
      if (live.current === scope && sequence.current === ticket)
        setView({
          scope,
          loading: false,
          error: error.message || "Approval record could not be loaded.",
        });
    }
  }
  if (invoice.status !== "approved") return null;
  if (!summary) return <small>Review details unavailable</small>;
  return (
    <>
      <button className="text-link" onClick={open}>
        View approval
      </button>
      {current &&
        createPortal(
          <Modal
            title={
              <span className="invoice-approval-title">
                Invoice {invoice.number} approval
              </span>
            }
            close={close}
          >
            <div className="invoice-approval-record">
              {current.loading ? (
                <p role="status">Loading approval record…</p>
              ) : current.error ? (
                <div role="alert">
                  <p>{current.error}</p>
                  <button className="secondary" onClick={open}>
                    Retry approval record
                  </button>
                </div>
              ) : (
                <>
                  <p>
                    Recorded review and approval. Later shipment updates do not
                    change this decision record.
                  </p>
                  <dl>
                    <div>
                      <dt>Entry reviewed by</dt>
                      <dd>{evidence.entry_reviewed_by}</dd>
                      <dd>
                        <RecordedTime value={evidence.entry_reviewed_at} />
                      </dd>
                    </div>
                    <div>
                      <dt>Approved by</dt>
                      <dd>{evidence.approved_by}</dd>
                      <dd>
                        <RecordedTime value={evidence.approved_at} />
                      </dd>
                    </div>
                    <div>
                      <dt>Carrier invoice evidence reference</dt>
                      <dd>{evidence.evidence_reference}</dd>
                    </div>
                  </dl>
                  <h3>Entry review findings</h3>
                  <p>{evidence.entry_review_note}</p>
                  <h3>Independent approval findings</h3>
                  <p>{evidence.approval_note}</p>
                </>
              )}
            </div>
          </Modal>,
          document.body,
        )}
    </>
  );
}
