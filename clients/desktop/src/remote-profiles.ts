const STORAGE_KEY = "orbcode.desktop.ssh-profiles.v1";
const MAX_PROFILES = 20;
const MAX_FIELD_LENGTH = 512;

export interface RemoteProfile {
  name: string;
  target: string;
  remoteCwd?: string;
  remoteOrbcodePath?: string;
}

export class RemoteProfileStore {
  constructor(private readonly storage: Storage) {}

  list(): RemoteProfile[] {
    const raw = this.storage.getItem(STORAGE_KEY);
    if (!raw) return [];
    try {
      const parsed = JSON.parse(raw) as unknown;
      if (!Array.isArray(parsed)) return [];
      return parsed
        .map(sanitizeProfile)
        .filter((profile): profile is RemoteProfile => profile !== undefined)
        .slice(0, MAX_PROFILES);
    } catch {
      return [];
    }
  }

  save(profile: RemoteProfile): RemoteProfile[] {
    const safe = sanitizeProfile(profile);
    if (!safe) throw new Error("Profile name and SSH target are required");
    const profiles = this.list().filter((item) => item.name !== safe.name);
    profiles.unshift(safe);
    const limited = profiles.slice(0, MAX_PROFILES);
    this.storage.setItem(STORAGE_KEY, JSON.stringify(limited));
    return limited;
  }

  remove(name: string): RemoteProfile[] {
    const profiles = this.list().filter((profile) => profile.name !== name);
    this.storage.setItem(STORAGE_KEY, JSON.stringify(profiles));
    return profiles;
  }
}

function sanitizeProfile(value: unknown): RemoteProfile | undefined {
  if (!value || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  const name = safeString(record.name);
  const target = safeString(record.target);
  if (!name || !target) return undefined;
  const remoteCwd = safeString(record.remoteCwd);
  const remoteOrbcodePath = safeString(record.remoteOrbcodePath);
  return {
    name,
    target,
    ...(remoteCwd ? { remoteCwd } : {}),
    ...(remoteOrbcodePath ? { remoteOrbcodePath } : {}),
  };
}

function safeString(value: unknown): string {
  return typeof value === "string" ? value.trim().slice(0, MAX_FIELD_LENGTH) : "";
}
