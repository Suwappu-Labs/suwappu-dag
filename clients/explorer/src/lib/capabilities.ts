// Capability-status document, served as a committed static file at
// `/capabilities.json`. A sober, factual inventory of what the protocol
// has shipped, has in progress, and has planned — not a marketing surface.

/** shipped = implemented + reviewed + CI-green; planned = designed, gated. */
export type CapabilityStatus = "shipped" | "in-progress" | "planned";

export interface CapabilityItem {
  name: string;
  status: CapabilityStatus;
  note: string;
  /** Repo-relative doc path, joined onto `repoBase` for the docs link. */
  doc?: string;
  /** When true, this capability is a load-bearing invariant of the protocol. */
  invariant?: boolean;
}

export interface CapabilityGroup {
  title: string;
  items: CapabilityItem[];
}

export interface CapabilitiesDoc {
  asOf: string;
  /** Base URL that `item.doc` paths are resolved against. */
  repoBase: string;
  note?: string;
  groups: CapabilityGroup[];
}

/**
 * Fetch the committed `/capabilities.json`. Throws on a network failure
 * or a non-2xx response; the caller renders that as an error state.
 */
export async function fetchCapabilities(): Promise<CapabilitiesDoc> {
  const resp = await fetch("/capabilities.json", { cache: "no-store" });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
  return (await resp.json()) as CapabilitiesDoc;
}
