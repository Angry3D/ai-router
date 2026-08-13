import { getVersion } from "@tauri-apps/api/app";

export async function getRunningAppVersion(): Promise<string | null> {
  try {
    return await getVersion();
  } catch {
    return null;
  }
}
