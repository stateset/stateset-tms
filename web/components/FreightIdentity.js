import { createContext, useContext } from "react";
const basic = {
  mode: "credential",
  fetch: (...args) => globalThis.fetch(...args),
  href: (path) => path,
  signOut: null,
};
export const FreightIdentityContext = createContext(basic);
export const useFreightIdentity = () => useContext(FreightIdentityContext);
