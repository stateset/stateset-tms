// Controlled remote ClerkJS/UI boundary for the REAL installed React SDK.
// No ClerkProvider, hook, Freight component or gateway is replaced. Browser
// tokens come from the owned RSA-signed backend fixture through Node bindings.
function installOwnedClerkClient(initial) {
  const subscribers = new Set();
  const events = new Map();
  const state = {
    org: initial.organization,
    sid: "sess_sdk_current",
    status: initial.status || "active",
    signedIn: true,
    offline: false,
  };
  const calls = {
    loads: [],
    tokens: [],
    userButtons: [],
    organizationSwitchers: [],
    signOut: [],
    tokenFailures: 0,
  };
  const selectors = new Set();
  let finishLoad;
  const clerk = {
    loaded: false,
    version: "owned-remote-client-protocol-fixture",
    instanceType: "development",
    isStandardBrowser: true,
    frontendApi: "clerk.fixture.test",
    telemetry: { record() {} },
    __internal_updateProps() {},
    __experimental_prefetchOrganizationSwitcher() {
      calls.organizationPrefetches = (calls.organizationPrefetches || 0) + 1;
      return Promise.resolve();
    },
    on(event, callback, options) {
      if (!events.has(event)) events.set(event, new Set());
      events.get(event).add(callback);
      if (event === "status" && options?.notify)
        callback(clerk.loaded ? "ready" : "loading");
    },
    off(event, callback) {
      events.get(event)?.delete(callback);
    },
    addListener(callback, options) {
      subscribers.add(callback);
      if (!options?.skipInitialEmit && clerk.__internal_lastEmittedResources)
        callback(clerk.__internal_lastEmittedResources);
      return () => subscribers.delete(callback);
    },
    async load(options) {
      calls.loads.push({
        sdkMetadata: options.sdkMetadata,
        afterSignOutUrl: options.afterSignOutUrl,
        telemetry: options.telemetry,
        uiConstructorType: typeof options.ui?.ClerkUI,
      });
      await new Promise((resolve) => {
        finishLoad = () => {
          clerk.loaded = true;
          publish();
          resolve();
        };
      });
    },
    async setActive(options) {
      const next = options.organization || null;
      await window.__ownedClerkControl({ org: next || state.org });
      state.org = next;
      publish();
    },
    async signOut(options = {}) {
      calls.signOut.push({
        redirectUrl: options.redirectUrl || calls.loads.at(-1)?.afterSignOutUrl,
      });
      await window.__ownedClerkControl({ status: "revoked" });
      state.signedIn = false;
      document.cookie = "__session=; Max-Age=0; Path=/; SameSite=Strict";
      publish();
      // Capture redirect configuration without pretending this fixture is the
      // hosted vendor UI or qualifying an external navigation/account logout.
    },
    mountUserButton(node, props) {
      calls.userButtons.push({
        hasAfterSignOutUrl: Object.prototype.hasOwnProperty.call(
          props,
          "afterSignOutUrl",
        ),
        afterSignOutUrl: props.afterSignOutUrl,
      });
      const button = document.createElement("button");
      button.textContent = "Owned identity sign out";
      button.onclick = () => clerk.signOut();
      node.replaceChildren(button);
    },
    unmountUserButton(node) {
      node.replaceChildren();
    },
    mountOrganizationSwitcher(node, props) {
      calls.organizationSwitchers.push({
        hidePersonal: props.hidePersonal,
        afterSelectOrganizationUrl: props.afterSelectOrganizationUrl,
      });
      const select = document.createElement("select");
      select.setAttribute("aria-label", "Owned identity organization");
      for (const id of ["org_sdk_staff", "org_sdk_other", "org_sdk_carrier"]) {
        const option = document.createElement("option");
        option.value = id;
        option.textContent = id;
        select.append(option);
      }
      select.value = state.org;
      select.onchange = () => clerk.setActive({ organization: select.value });
      selectors.add(select);
      node.replaceChildren(select);
    },
    unmountOrganizationSwitcher(node) {
      for (const child of node.children) selectors.delete(child);
      node.replaceChildren();
    },
    mountSignIn(node, props) {
      const heading = document.createElement("h2");
      heading.textContent = "Owned identity sign-in boundary";
      node.replaceChildren(heading);
      calls.signInProps = {
        routing: props.routing,
        forceRedirectUrl: props.forceRedirectUrl,
      };
    },
    unmountSignIn(node) {
      node.replaceChildren();
    },
  };
  function publish() {
    clerk.user = state.signedIn
      ? {
          id: "user_staff",
          organizationMemberships: [
            "org_sdk_staff",
            "org_sdk_other",
            "org_sdk_carrier",
          ].map((id) => ({
            organization: { id },
            role: "org:admin",
            permissions: [],
          })),
        }
      : null;
    clerk.organization =
      state.signedIn && state.org ? { id: state.org, slug: state.org } : null;
    clerk.session = state.signedIn
      ? {
          id: state.sid,
          status: state.status,
          actor: null,
          factorVerificationAge: [0, 0],
          lastActiveToken: {
            jwt: {
              claims: {
                sub: "user_staff",
                sid: state.sid,
                v: 2,
                o: { id: state.org, rol: "admin" },
              },
            },
          },
          async getToken(options = {}) {
            calls.tokens.push({
              organizationId: options.organizationId,
              sessionId: state.sid,
            });
            if (state.offline) {
              calls.tokenFailures++;
              const error = new Error(
                "Owned identity token service unavailable",
              );
              error.name = "ClerkOfflineError";
              throw error;
            }
            const token = await window.__ownedClerkToken({
              sid: state.sid,
              o: { id: options.organizationId || state.org, rol: "admin" },
            });
            document.cookie = `__session=${token}; Path=/; SameSite=Strict`;
            return token;
          },
        }
      : null;
    clerk.client = {
      id: "owned-sdk-client",
      sessions: clerk.session ? [clerk.session] : [],
    };
    clerk.isSignedIn = !!clerk.session;
    clerk.__internal_lastEmittedResources = {
      client: clerk.client,
      session: clerk.session,
      user: clerk.user,
      organization: clerk.organization,
    };
    for (const select of selectors) select.value = state.org;
    for (const callback of subscribers)
      callback(clerk.__internal_lastEmittedResources);
    for (const callback of events.get("status") || [])
      callback(clerk.loaded ? "ready" : "loading");
  }
  publish();
  window.Clerk = clerk;
  window.__internal_ClerkUICtor = function OwnedRemoteUIBoundary() {};
  window.__ownedClerk = {
    calls,
    finishLoad() {
      finishLoad();
    },
    setOffline(value) {
      state.offline = value;
    },
    async setOrganization(id) {
      await clerk.setActive({ organization: id });
    },
    async replaceSession() {
      await window.__ownedClerkControl({ status: "active", org: state.org });
      state.sid = "sess_sdk_next";
      publish();
    },
    async activate() {
      await window.__ownedClerkControl({ status: "active" });
      state.status = "active";
      publish();
    },
  };
}
module.exports = { installOwnedClerkClient };
