import { useState } from "react";
import {
  AlertTriangle,
  ExternalLink,
  Inbox,
  RefreshCw,
  Search,
} from "lucide-react";

const title = (value) =>
  value.replaceAll("_", " ").replace(/\b\w/g, (c) => c.toUpperCase());
export function InboundDocuments({
  items = [],
  busy,
  inspect,
  retry,
  canRetry,
  reviewLoad,
}) {
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const visible = [...items]
    .reverse()
    .filter(
      (item) =>
        (filter === "all" ||
          (filter === "attention"
            ? [
                "retry",
                "held",
                "reconciliation_required",
                "finance_review",
              ].includes(item.status)
            : filter === "queued"
              ? ["pending", "processing"].includes(item.status)
              : item.status === filter)) &&
        `${item.document_id} ${item.notification.partner_id} ${item.notification.reference || ""} ${item.notification.transaction_set}`
          .toLowerCase()
          .includes(search.toLowerCase()),
    );
  return (
    <section className="panel inbound-panel">
      <div className="panel-heading">
        <div>
          <h2>
            Inbound EDI documents{" "}
            <span className="subtle-count">{items.length}</span>
          </h2>
          <p>
            Received securely, retained until processed, and recoverable after
            interruptions.
          </p>
        </div>
        <Inbox size={19} className="muted" />
      </div>
      <div className="table-toolbar">
        <div className="search">
          <Search size={16} />
          <input
            aria-label="Search inbound documents"
            value={search}
            placeholder="Document, partner, shipment, or transaction…"
            onChange={(event) => setSearch(event.target.value)}
          />
        </div>
        <select
          className="role-select"
          aria-label="Filter inbound documents"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        >
          <option value="all">All documents</option>
          <option value="attention">Needs attention</option>
          <option value="queued">Queued / processing</option>
          <option value="applied">Applied</option>
          <option value="ignored">Ignored</option>
          <option value="reconciliation_required">
            Carrier reconciliation required
          </option>
          <option value="reconciled">Reconciled</option>
          <option value="finance_review">
            Finance classification required
          </option>
          <option value="finance_reviewed">Finance reviewed</option>
        </select>
      </div>
      {visible.length ? (
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Document / partner</th>
                <th>Transaction</th>
                <th>Received</th>
                <th>Status</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((item) => (
                <tr key={item.id}>
                  <td>
                    <strong>{item.document_id}</strong>
                    <small>
                      {item.notification.partner_id}
                      {item.notification.reference
                        ? ` · ${item.notification.reference}`
                        : ""}
                    </small>
                  </td>
                  <td>X12 {item.notification.transaction_set}</td>
                  <td>
                    {new Date(item.received_at).toLocaleString()}
                    <small>
                      {item.total_attempts ?? item.attempts} processing attempts
                    </small>
                  </td>
                  <td>
                    <span className={`badge ${item.status}`}>
                      <span className="badge-dot" />
                      {title(item.status)}
                    </span>
                    {item.last_error && (
                      <small className="red inbound-error">
                        <AlertTriangle size={11} />
                        {item.last_error}
                      </small>
                    )}
                  </td>
                  <td>
                    <div className="toolbar-actions">
                      {item.status === "reconciliation_required" && (
                        <button
                          className="secondary"
                          onClick={() => reviewLoad(item.result?.data?.load_id)}
                        >
                          Review carrier conflict
                        </button>
                      )}
                      {["retry", "held"].includes(item.status) && (
                        <button
                          className="secondary"
                          disabled={busy || !canRetry}
                          aria-label={`Retry inbound document ${item.document_id}`}
                          onClick={() => retry(item)}
                        >
                          <RefreshCw size={13} />
                          Retry
                        </button>
                      )}
                      {item.status === "finance_review" && (
                        <button
                          className="secondary"
                          onClick={() => reviewLoad(item.result?.data?.load_id)}
                        >
                          Review carrier invoice
                        </button>
                      )}
                      <button
                        className="icon-button"
                        aria-label={`Inspect inbound document ${item.document_id}`}
                        onClick={() => inspect(item)}
                      >
                        <ExternalLink size={16} />
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="empty">
          <Inbox size={28} />
          <h3>
            {items.length
              ? "No matching documents"
              : "Your inbound queue is clear"}
          </h3>
          <p>
            {items.length
              ? "Try another status or search term."
              : "Signed carrier callbacks appear here before document processing begins."}
          </p>
        </div>
      )}
    </section>
  );
}

export function InboundDocumentDetail({ item, busy, retry, canRetry }) {
  return (
    <div className="payload">
      <span className={`badge ${item.status}`}>
        <span className="badge-dot" />
        {title(item.status)}
      </span>
      <dl className="detail-info">
        <div>
          <dt>EDI document</dt>
          <dd>{item.document_id}</dd>
        </div>
        <div>
          <dt>Trading partner</dt>
          <dd>{item.notification.partner_id}</dd>
        </div>
        <div>
          <dt>Received</dt>
          <dd>{new Date(item.received_at).toLocaleString()}</dd>
        </div>
        <div>
          <dt>Processing attempts</dt>
          <dd>{item.total_attempts ?? item.attempts}</dd>
        </div>
        <div>
          <dt>Last updated</dt>
          <dd>{new Date(item.updated_at).toLocaleString()}</dd>
        </div>
        <div>
          <dt>Next automatic retry</dt>
          <dd>
            {item.status === "retry" && item.next_attempt_at
              ? new Date(item.next_attempt_at).toLocaleString()
              : "—"}
          </dd>
        </div>
      </dl>
      {item.last_error && (
        <div className="error-banner" role="status">
          <AlertTriangle size={16} />
          {item.last_error}
        </div>
      )}
      {["retry", "held"].includes(item.status) && (
        <button
          className="secondary"
          disabled={busy || !canRetry}
          onClick={() => retry(item)}
        >
          <RefreshCw size={14} />
          Retry processing
        </button>
      )}
      {item.history?.length > 0 && (
        <>
          <h3>Processing history</h3>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Attempt</th>
                  <th>Finished</th>
                  <th>Outcome</th>
                  <th>Details</th>
                </tr>
              </thead>
              <tbody>
                {item.history.map((entry, index) => (
                  <tr key={index}>
                    <td>{entry.number}</td>
                    <td>{new Date(entry.finished_at).toLocaleString()}</td>
                    <td>{title(entry.outcome)}</td>
                    <td className="inbound-history-error">
                      {entry.error || "Completed"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
      <h3 className="detail-history">Original authenticated notification</h3>
      <p>
        The original callback bytes remain unchanged. Callback receipt is
        separate from successful business processing.
      </p>
      <pre>{item.original_body}</pre>
      {item.normalized_event && (
        <>
          <h3>Canonical event</h3>
          <pre>{JSON.stringify(item.normalized_event, null, 2)}</pre>
        </>
      )}
      {item.result && (
        <>
          <h3>Processing result</h3>
          <pre>{JSON.stringify(item.result, null, 2)}</pre>
        </>
      )}
      <h3>Callback evidence</h3>
      <pre>
        {JSON.stringify(
          {
            delivery_ids: item.delivery_ids,
            key_version: item.key_version,
            original_authentication:
              item.authentication || "Not retained by an older receipt",
          },
          null,
          2,
        )}
      </pre>
    </div>
  );
}
