// Shared by the owned services, signed identity fixture and browser clients.
module.exports = function serviceOrigins(
  raw = process.env.FREIGHT_E2E_PORT_BASE,
) {
  let staffPort = 3015,
    clerkPort = 3016,
    apiPort = 8095,
    fixturePort = 8097;
  if (raw !== undefined && raw !== "") {
    const base = Number(raw);
    if (
      typeof raw !== "string" ||
      !/^\d+$/.test(raw) ||
      !Number.isInteger(base) ||
      base < 1024 ||
      base > 65532
    )
      throw new Error(
        "FREIGHT_E2E_PORT_BASE must be an integer from 1024 to 65532",
      );
    [staffPort, clerkPort, apiPort, fixturePort] = [
      base,
      base + 1,
      base + 2,
      base + 3,
    ];
  }
  return Object.freeze({
    staffPort,
    clerkPort,
    apiPort,
    fixturePort,
    staffOrigin: `http://127.0.0.1:${staffPort}`,
    clerkOrigin: `http://127.0.0.1:${clerkPort}`,
    apiOrigin: `http://127.0.0.1:${apiPort}`,
    fixtureOrigin: `http://127.0.0.1:${fixturePort}`,
  });
};
