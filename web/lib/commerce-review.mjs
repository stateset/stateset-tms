export function externalReviewIntent(job, load, preview, fields) {
  if (
    !job ||
    !load ||
    !preview ||
    typeof job.id !== "string" ||
    typeof load.id !== "string" ||
    typeof load.sync_order_id !== "string" ||
    !/^[A-Za-z0-9_-]{1,200}$/.test(job.id) ||
    !/^[A-Za-z0-9_-]{1,200}$/.test(load.id) ||
    job.destination !== "stateset-icommerce" ||
    job.status !== "held" ||
    job.lease ||
    job.leased_until ||
    job.load_id !== load.id ||
    preview.job_id !== job.id ||
    preview.load_id !== load.id ||
    preview.expected_load_version !== load.version ||
    preview.expected_total_attempts !== job.total_attempts ||
    !Number.isSafeInteger(load.version) ||
    load.version < 0 ||
    !Number.isSafeInteger(job.total_attempts) ||
    job.total_attempts < 0 ||
    preview.late_original_fenced !== true ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      preview.shipment?.id,
    ) ||
    !Number.isInteger(job.payload?.plan?.expected_version) ||
    job.payload.plan.expected_version < 0 ||
    preview.shipment?.id !== job.payload?.plan?.shipment_id ||
    preview.shipment?.order_id !== load.sync_order_id ||
    preview.original_plan?.expected_version !==
      job.payload?.plan?.expected_version ||
    !Number.isInteger(preview.shipment?.version) ||
    preview.shipment.version > 2147483647 ||
    preview.shipment.version <= job.payload?.plan?.expected_version
  )
    throw new Error("Handoff or shipment review changed; preview again.");
  const evidence = {};
  for (const field of ["contact", "reference", "note"]) {
    const value = fields?.[field];
    if (
      typeof value !== "string" ||
      !value.trim() ||
      new TextEncoder().encode(value).length > 200 ||
      /[\x00-\x1f\x7f-\x9f]/.test(value)
    )
      throw new Error(
        "Contact, reference and note require 1–200 bytes without control characters.",
      );
    evidence[field] = value.trim();
  }
  return {
    path: `outbox/${job.id}/commerce-review`,
    body: {
      expected_load_version: load.version,
      expected_total_attempts: job.total_attempts,
      expected_shipment_version: preview.shipment.version,
      ...evidence,
    },
  };
}
