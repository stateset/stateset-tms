import Head from "next/head";
import dynamic from "next/dynamic";
import { useCallback, useEffect, useRef, useState } from "react";
import { publicAuthConfig } from "../lib/auth-config.mjs";
import { useFreightIdentity } from "../components/FreightIdentity";

const ClerkWorkspace = dynamic(() => import("../components/ClerkWorkspace"), {
  ssr: false,
});
const statuses = [
  "planned",
  "tendered",
  "accepted",
  "dispatched",
  "in_transit",
  "delivered",
  "cancelled",
];
const label = (value) => value.replaceAll("_", " ");
export async function getServerSideProps({ res }) {
  res.setHeader("Cache-Control", "no-store");
  return { props: { authConfig: publicAuthConfig() } };
}
export default function CatalogPage({ authConfig }) {
  return authConfig?.mode === "clerk" ? (
    <ClerkWorkspace
      publishableKey={authConfig.publishableKey}
      destination="/catalog"
    >
      <Catalog />
    </ClerkWorkspace>
  ) : (
    <Catalog />
  );
}
function Catalog() {
  const identity = useFreightIdentity();
  const [data, setData] = useState(null),
    [error, setError] = useState(""),
    [login, setLogin] = useState(false),
    [loading, setLoading] = useState(true),
    [query, setQuery] = useState(""),
    [status, setStatus] = useState(""),
    [filters, setFilters] = useState({ q: "", status: "" }),
    [cursor, setCursor] = useState(null),
    [previous, setPrevious] = useState([]),
    [refresh, setRefresh] = useState(0),
    [busy, setBusy] = useState(false),
    [signOutFailed, setSignOutFailed] = useState(false);
  const generation = useRef(0),
    blocked = useRef(false),
    abort = useRef(null);
  const close = useCallback((message = "") => {
    blocked.current = true;
    generation.current++;
    abort.current?.abort();
    setData(null);
    setLogin(true);
    setLoading(false);
    setError(message);
    setQuery("");
    setStatus("");
    setFilters({ q: "", status: "" });
    setCursor(null);
    setPrevious([]);
  }, []);
  useEffect(() => {
    if (blocked.current) return;
    const current = ++generation.current,
      controller = new AbortController();
    abort.current = controller;
    setLoading(true);
    setData(null);
    setError("");
    const params = new URLSearchParams({ limit: "25" });
    if (filters.q) params.set("q", filters.q);
    if (filters.status) params.set("status", filters.status);
    if (cursor) params.set("cursor", cursor);
    (async () => {
      try {
        const response = await identity.fetch(`/api/freight/loads?${params}`, {
          signal: controller.signal,
        });
        const result = await response.json();
        if (current !== generation.current || blocked.current) return;
        if (response.status === 401 || response.status === 403) {
          close(result.error?.message || "Access is unavailable.");
          return;
        }
        if (!response.ok)
          throw new Error(result.error?.message || "Shipments could not load.");
        setData(result);
        setLogin(false);
      } catch (e) {
        if (
          current === generation.current &&
          !blocked.current &&
          e.name !== "AbortError"
        )
          setError(e.message);
      } finally {
        if (current === generation.current && !blocked.current)
          setLoading(false);
      }
    })();
    return () => {
      generation.current++;
      controller.abort();
    };
  }, [identity.fetch, filters, cursor, refresh, close]);
  const signOut = async () => {
    if (busy) return;
    close();
    setBusy(true);
    try {
      if (identity.signOut) await identity.signOut();
      else {
        const response = await fetch("/api/session", { method: "DELETE" });
        if (!response.ok)
          throw new Error("Sign out could not be confirmed. Retry sign out.");
      }
      setSignOutFailed(false);
    } catch (e) {
      setSignOutFailed(true);
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };
  if (login)
    return (
      <main className="login catalog-login">
        <section className="login-card">
          <h1>Shipment catalog</h1>
          <p>
            {identity.mode === "clerk"
              ? "Your access needs to be checked again."
              : "Sign in with your workspace credential."}
          </p>
          {error && (
            <p role="alert" className="form-error">
              {error}
            </p>
          )}
          {identity.mode === "clerk" ? (
            <button
              className="primary"
              disabled={busy || signOutFailed}
              onClick={() => window.location.reload()}
            >
              Check access again
            </button>
          ) : (
            <form
              onSubmit={async (e) => {
                e.preventDefault();
                if (busy || signOutFailed) return;
                const token = new FormData(e.currentTarget).get("token"),
                  current = generation.current;
                setBusy(true);
                setError("");
                try {
                  const response = await fetch("/api/session", {
                    method: "POST",
                    headers: { "Content-Type": "application/json" },
                    body: JSON.stringify({ token }),
                  });
                  const result = await response.json();
                  if (current !== generation.current) return;
                  if (!response.ok)
                    throw new Error(result.error?.message || "Sign in failed.");
                  blocked.current = false;
                  setLogin(false);
                  setRefresh((value) => value + 1);
                } catch (e) {
                  if (current === generation.current) setError(e.message);
                } finally {
                  if (current === generation.current) setBusy(false);
                }
              }}
            >
              <label>
                Workspace credential
                <input
                  name="token"
                  type="password"
                  autoComplete="current-password"
                  required
                  disabled={busy || signOutFailed}
                />
              </label>
              <button className="primary" disabled={busy || signOutFailed}>
                Open catalog
              </button>
            </form>
          )}
          {signOutFailed && (
            <button className="secondary" disabled={busy} onClick={signOut}>
              Retry sign out
            </button>
          )}
        </section>
      </main>
    );
  return (
    <>
      <Head>
        <title>Shipment catalog · StateSet Freight</title>
      </Head>
      <main className="catalog-page">
        <header className="catalog-header">
          <a href="/" className="brand">
            stateset<span className="brand-product">freight</span>
          </a>
          <div>
            <a href="/">Operations workspace</a>
            <button className="secondary" onClick={signOut} disabled={busy}>
              Sign out
            </button>
          </div>
        </header>
        <section className="catalog-intro">
          <p className="eyebrow">Shipment discovery</p>
          <h1>Shipment catalog</h1>
          <p>
            Find a shipment by reference, order, city, or reported location.
            Open its workspace record to review agreements, documents, and
            operational holds.
          </p>
        </section>
        <form
          className="catalog-filters"
          onSubmit={(e) => {
            e.preventDefault();
            setCursor(null);
            setPrevious([]);
            setFilters({ q: query.trim(), status });
          }}
        >
          <label>
            Find shipments
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              maxLength={200}
              placeholder="Reference, order, city, or location"
            />
          </label>
          <label>
            Shipment status
            <select value={status} onChange={(e) => setStatus(e.target.value)}>
              <option value="">All statuses</option>
              {statuses.map((value) => (
                <option key={value} value={value}>
                  {label(value)}
                </option>
              ))}
            </select>
          </label>
          <button className="primary" type="submit">
            Search shipments
          </button>
          <button
            className="secondary"
            type="button"
            onClick={() => setRefresh((value) => value + 1)}
          >
            Refresh
          </button>
        </form>
        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        <section
          className="panel"
          aria-label="Shipment results"
          aria-busy={loading}
        >
          {loading ? (
            <p role="status" className="catalog-message">
              Loading shipments…
            </p>
          ) : (
            data && (
              <>
                <div className="table-scroll">
                  <table>
                    <thead>
                      <tr>
                        <th>Shipment</th>
                        <th>Route</th>
                        <th>Pickup / delivery</th>
                        <th>Status</th>
                        <th>Reported location</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.loads.map((load) => (
                        <tr key={load.id}>
                          <td>
                            <a
                              className="text-link"
                              href={`/?load=${encodeURIComponent(load.id)}`}
                            >
                              {load.reference}
                            </a>
                            <small>
                              {load.mode} · {load.equipment}
                            </small>
                          </td>
                          <td>
                            <span>
                              {load.origin.city}, {load.origin.state}
                            </span>
                            <small>
                              {load.destination.city}, {load.destination.state}
                            </small>
                          </td>
                          <td>
                            <span>{load.origin.date}</span>
                            <small>{load.destination.date}</small>
                          </td>
                          <td>
                            <span className={`badge ${load.status}`}>
                              {label(load.status)}
                            </span>
                            {load.exception && (
                              <small className="exception-label">
                                Reported exception
                              </small>
                            )}
                          </td>
                          <td>{load.location || "No location reported"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                {!data.loads.length && (
                  <p className="catalog-message">
                    No shipments match these filters.
                  </p>
                )}
                <nav className="catalog-pagination" aria-label="Shipment pages">
                  <button
                    className="secondary"
                    disabled={!previous.length}
                    onClick={() => {
                      setCursor(previous[previous.length - 1]);
                      setPrevious((values) => values.slice(0, -1));
                    }}
                  >
                    Previous page
                  </button>
                  <span role="status">
                    Page {previous.length + 1} · {data.loads.length} shipments
                  </span>
                  <button
                    className="secondary"
                    disabled={!data.next_cursor}
                    onClick={() => {
                      setPrevious((values) => [...values, cursor]);
                      setCursor(data.next_cursor);
                    }}
                  >
                    Next page
                  </button>
                </nav>
              </>
            )
          )}
        </section>
      </main>
    </>
  );
}
