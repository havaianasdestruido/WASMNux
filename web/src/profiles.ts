/**
 * The two Linux profiles. Profiles are fully isolated: separate disk ids,
 * separate guest-asset directories, separate snapshots. The data here is
 * the single place profile constants live — no duplicated literals spread
 * across the UI or worker code.
 */

export type ProfileId = "vanilla" | "Debian";

export interface Profile {
  readonly id: ProfileId;
  readonly displayName: string;
  readonly description: string;
  /** Directory under webroot holding built guest assets for this profile. */
  readonly assetPath: string;
  /** Storage disk id (ChunkStore namespace) — isolation boundary. */
  readonly diskId: string;
  readonly ramMiB: number;
  readonly defaultDiskMiB: number;
  readonly bootargs: string;
}

export const PROFILES: Readonly<Record<ProfileId, Profile>> = {
  vanilla: {
    id: "vanilla",
    displayName: "Vanilla Linux",
    description:
      "Latest stable upstream Linux kernel + BusyBox initramfs. Small, fast to boot, persistent data disk.",
    assetPath: "guest/vanilla",
    diskId: "profiles-vanilla",
    ramMiB: 256,
    defaultDiskMiB: 256,
    bootargs: "console=ttyS0 earlycon=sbi rw root=/dev/vda rootfstype=ext4 panic=-1",
  },
  Debian: {
    id: "Debian",
    displayName: "Debian",
    description:
      "Latest stable Debian (riscv64) root filesystem with apt/dpkg. Requires the CPU F/D milestone (see limitations).",
    assetPath: "guest/Debian",
    diskId: "profiles-Debian",
    ramMiB: 512,
    defaultDiskMiB: 2048,
    bootargs: "console=ttyS0 earlycon=sbi rw root=/dev/vda rootfstype=ext4 panic=-1",
  },
};

export const PROFILE_IDS: readonly ProfileId[] = ["vanilla", "Debian"];

/** Runtime-validates untrusted input (URL params, messages, storage). */
export function parseProfileId(value: unknown): ProfileId | undefined {
  return value === "vanilla" || value === "Debian" ? value : undefined;
}

export function profileById(id: ProfileId): Profile {
  return PROFILES[id];
}

/**
 * Guest asset manifest filenames, as produced by the guest build pipeline
 * (see guest/scripts). The app refuses to "boot" a profile whose real
 * assets are absent — see assets.ts.
 */
export interface GuestManifest {
  readonly profile: ProfileId;
  readonly kernelVersion: string;
  readonly kernelImage: string;
  /** OpenSBI fw_jump ELF, when the firmware boot path is used. */
  readonly firmwareImage?: string;
  readonly initrdImage?: string;
  /** Prebuilt root disk image served on first boot (Debian profile). */
  readonly diskImage?: string;
  readonly builtAt: string;
  readonly notes?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function optionalString(value: unknown, field: string): string | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`guest manifest field "${field}" must be a non-empty string when present`);
  }
  return value;
}

/**
 * Validates a fetched guest manifest. Filenames must be bare names (no
 * paths): the app composes the URL from `profile.assetPath` + name, so a
 * hostile or corrupted manifest cannot redirect fetches elsewhere.
 */
export function parseGuestManifest(value: unknown, expected: ProfileId): GuestManifest {
  if (!isRecord(value)) {
    throw new Error("guest manifest is not an object");
  }
  const profile = parseProfileId(value["profile"]);
  if (profile !== expected) {
    throw new Error(`guest manifest profile mismatch: expected ${expected}`);
  }
  const strings = ["kernelVersion", "kernelImage", "builtAt"] as const;
  for (const field of strings) {
    if (typeof value[field] !== "string" || (value[field] as string).length === 0) {
      throw new Error(`guest manifest field "${field}" must be a non-empty string`);
    }
  }
  const filenameFields = ["kernelImage", "firmwareImage", "initrdImage", "diskImage"] as const;
  const names = new Map<string, string | undefined>();
  for (const field of filenameFields) {
    const name = optionalString(value[field], field);
    if (name !== undefined && (name.includes("/") || name.includes("\\") || name === "..")) {
      throw new Error(`guest manifest field "${field}" must be a bare filename`);
    }
    names.set(field, name);
  }
  const manifest: GuestManifest = {
    profile,
    kernelVersion: value["kernelVersion"] as string,
    kernelImage: names.get("kernelImage") as string,
    builtAt: value["builtAt"] as string,
  };
  const firmware = names.get("firmwareImage");
  const initrd = names.get("initrdImage");
  const disk = names.get("diskImage");
  const notes = optionalString(value["notes"], "notes");
  return {
    ...manifest,
    ...(firmware !== undefined ? { firmwareImage: firmware } : {}),
    ...(initrd !== undefined ? { initrdImage: initrd } : {}),
    ...(disk !== undefined ? { diskImage: disk } : {}),
    ...(notes !== undefined ? { notes } : {}),
  };
}
