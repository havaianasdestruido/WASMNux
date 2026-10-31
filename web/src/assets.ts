/**
 * Guest asset discovery. A profile can only boot with real guest binaries
 * (kernel Image, firmware, initrd, root-disk seed) built by the pipeline
 * in guest/. When they are absent the app says so and does not pretend:
 * there is no fallback "demo mode" anywhere in this codebase.
 */

import { parseGuestManifest, type GuestManifest, type Profile } from "./profiles.js";

export type AssetState =
  | { readonly kind: "ready"; readonly manifest: GuestManifest }
  | { readonly kind: "missing"; readonly detail: string }
  | { readonly kind: "invalid"; readonly detail: string };

type FetchFn = (input: string) => Promise<Pick<Response, "ok" | "status" | "json" | "arrayBuffer">>;

const defaultFetch: FetchFn = (input) => fetch(input);

/** Probes `<assetPath>/manifest.json` and validates it structurally. */
export async function probeGuestAssets(
  profile: Profile,
  fetchFn: FetchFn = defaultFetch,
): Promise<AssetState> {
  const url = `/${profile.assetPath}/manifest.json`;
  let response: Awaited<ReturnType<FetchFn>>;
  try {
    response = await fetchFn(url);
  } catch (error) {
    return {
      kind: "missing",
      detail: `could not fetch ${url}: ${error instanceof Error ? error.message : String(error)}`,
    };
  }
  if (response.status === 404 || !response.ok) {
    return {
      kind: "missing",
      detail: `${url} returned HTTP ${response.status}. Build the guest assets with the pipeline in guest/scripts (or download the CI guest artifact) and place them under web/public/${profile.assetPath}/.`,
    };
  }
  try {
    const json: unknown = await response.json();
    return { kind: "ready", manifest: parseGuestManifest(json, profile.id) };
  } catch (error) {
    return {
      kind: "invalid",
      detail: error instanceof Error ? error.message : String(error),
    };
  }
}

/** Fetches one asset binary named by the (already validated) manifest. */
export async function fetchAssetBytes(
  profile: Profile,
  filename: string,
  fetchFn: FetchFn = defaultFetch,
): Promise<Uint8Array> {
  if (filename.includes("/") || filename.includes("\\") || filename === ".." || filename === "") {
    throw new Error(`refusing non-bare asset filename: ${JSON.stringify(filename)}`);
  }
  const url = `/${profile.assetPath}/${filename}`;
  const response = await fetchFn(url);
  if (!response.ok) {
    throw new Error(`fetching ${url} failed with HTTP ${response.status}`);
  }
  return new Uint8Array(await response.arrayBuffer());
}
