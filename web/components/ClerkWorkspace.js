import { forgetClerkJournalSession } from "../lib/save-journal.mjs";
import {
  ClerkProvider,
  SignIn,
  OrganizationSwitcher,
  UserButton,
  useAuth,
  useClerk,
} from "@clerk/react";
import { Component, useEffect, useMemo, useRef, useState } from "react";
import { FreightIdentityContext } from "./FreightIdentity";
import {
  organizationFetch,
  organizationHref,
} from "../lib/identity-client.mjs";

function MembershipGate({ children, destination, partner }) {
  const { isLoaded, isSignedIn, orgId, sessionId, getToken } = useAuth();
  const clerk = useClerk();
  const previousSession = useRef(null);
  useEffect(() => {
    if (!isLoaded) return;
    const previous = previousSession.current;
    previousSession.current = sessionId || null;
    if (previous && previous !== sessionId)
      forgetClerkJournalSession(previous).catch(() => {});
  }, [isLoaded, sessionId]);

  const [unavailable, setUnavailable] = useState(false);
  useEffect(() => {
    if (isLoaded) {
      setUnavailable(false);
      return;
    }
    const timer = setTimeout(() => setUnavailable(true), 12000);
    return () => clearTimeout(timer);
  }, [isLoaded]);
  const identity = useMemo(
    () => ({
      mode: "clerk",
      organization: orgId,
      session: sessionId,
      fetch: organizationFetch(orgId, getToken),
      href: (path) => organizationHref(path, orgId),
      signOut: async () => {
        const cleanup = forgetClerkJournalSession(sessionId).then(
          () => true,
          () => false,
        );
        await clerk.signOut({ redirectUrl: destination });
        return { recoveryCleared: await cleanup };
      },
    }),
    [orgId, sessionId, getToken, clerk, destination],
  );
  if (!isLoaded && unavailable) return <SignInUnavailable />;
  if (!isLoaded)
    return (
      <main className="login">
        <p role="status">Loading StateSet sign-in…</p>
      </main>
    );
  if (!isSignedIn)
    return (
      <main className="login">
        <section className="staff-login">
          <h1>StateSet Freight</h1>
          <p>
            Sign in with your StateSet {partner ? "partner" : "staff"} account.
          </p>
          <SignIn routing="hash" forceRedirectUrl={destination} />
          <a href={partner ? "/" : "/portal"}>
            {partner ? "Staff workspace" : "Carrier or customer access"}
          </a>
        </section>
      </main>
    );
  return (
    <>
      <div className="staff-identity-bar">
        <span>
          {partner ? "StateSet partner portal" : "StateSet staff workspace"}
        </span>
        <OrganizationSwitcher
          hidePersonal
          afterSelectOrganizationUrl={destination}
        />
        <UserButton />
      </div>
      {orgId ? (
        <FreightIdentityContext.Provider value={identity}>
          <div key={`${sessionId}:${orgId}`}>{children}</div>
        </FreightIdentityContext.Provider>
      ) : (
        <main className="login">
          <section className="login-card">
            <h1>Choose your organization</h1>
            <p>
              Select your approved StateSet organization to open its freight
              workspace.
            </p>
            <OrganizationSwitcher hidePersonal />
            <button className="secondary" onClick={identity.signOut}>
              Sign out
            </button>
          </section>
        </main>
      )}
    </>
  );
}
export default function ClerkWorkspace({
  publishableKey,
  children,
  destination = "/",
  partner = false,
}) {
  return (
    <IdentityBoundary>
      <ClerkProvider
        publishableKey={publishableKey}
        afterSignOutUrl={destination}
        telemetry={false}
      >
        <MembershipGate destination={destination} partner={partner}>
          {children}
        </MembershipGate>
      </ClerkProvider>
    </IdentityBoundary>
  );
}
function SignInUnavailable() {
  return (
    <main className="login">
      <section className="login-card">
        <h1>Staff sign-in unavailable</h1>
        <p>The identity service could not load. Retry when it is available.</p>
        <button className="primary" onClick={() => window.location.reload()}>
          Retry staff sign-in
        </button>
      </section>
    </main>
  );
}
class IdentityBoundary extends Component {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? <SignInUnavailable /> : this.props.children;
  }
}
