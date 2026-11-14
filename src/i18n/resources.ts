import type { Resource } from "i18next";

const CATALOG_PATH = /\/locales\/([^/]+)\/([^/]+)\.json$/;

export function buildResources(
  catalogs: Record<string, { default: Record<string, unknown> }>,
): { resources: Resource; namespaces: string[] } {
  const resources: Resource = {};
  const namespaces = new Set<string>();
  for (const [path, module] of Object.entries(catalogs)) {
    const match = CATALOG_PATH.exec(path);
    if (!match) continue;
    const [, locale, namespace] = match;
    resources[locale] = { ...resources[locale], [namespace]: module.default };
    namespaces.add(namespace);
  }
  return { resources, namespaces: [...namespaces].sort() };
}
