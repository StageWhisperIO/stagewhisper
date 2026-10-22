import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface Capabilities {
  diarization: boolean;
}

const emptyCapabilities: Capabilities = {
  diarization: false,
};

export function useCapabilities(): Capabilities {
  const [capabilities, setCapabilities] = useState<Capabilities>(emptyCapabilities);

  useEffect(() => {
    void invoke<Capabilities>("get_capabilities")
      .then(setCapabilities)
      .catch(() => setCapabilities(emptyCapabilities));
  }, []);

  return capabilities;
}

export function useCapability(name: keyof Capabilities): boolean {
  return useCapabilities()[name];
}
