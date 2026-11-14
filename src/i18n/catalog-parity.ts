import type { Resource } from "i18next";

const PLURAL_SUFFIX = /_(zero|one|two|few|many|other)$/;
const POLISH_PLURAL_FORMS = ["one", "few", "many", "other"];
const PLACEHOLDER = /\{\{\s*([\w.]+)[^}]*\}\}/g;
const TAG = /<\/?([\w]+)\s*\/?>/g;

type Flat = Map<string, string>;

function flatten(node: unknown, prefix: string, out: Flat): Flat {
  if (typeof node === "string") {
    out.set(prefix, node);
    return out;
  }
  if (node && typeof node === "object") {
    for (const [key, value] of Object.entries(node)) {
      flatten(value, prefix ? `${prefix}.${key}` : key, out);
    }
  }
  return out;
}

function baseKey(key: string): string {
  return key.replace(PLURAL_SUFFIX, "");
}

function collect(pattern: RegExp, text: string): string[] {
  return [...text.matchAll(pattern)].map((match) => match[1]);
}

function tokensByBase(flat: Flat, pattern: RegExp): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  for (const [key, text] of flat) {
    const base = baseKey(key);
    const tokens = out.get(base) ?? new Set<string>();
    for (const token of collect(pattern, text)) if (token !== "count") tokens.add(token);
    out.set(base, tokens);
  }
  return out;
}

function sameSet(a: Set<string>, b: Set<string>): boolean {
  return a.size === b.size && [...a].every((item) => b.has(item));
}

function pluralForms(flat: Flat, base: string): string[] {
  return [...flat.keys()]
    .filter((key) => baseKey(key) === base && PLURAL_SUFFIX.test(key))
    .map((key) => PLURAL_SUFFIX.exec(key)![1]);
}

function compareNamespace(namespace: string, english: Flat, translated: Flat, locale: string): string[] {
  const problems: string[] = [];
  const englishBases = new Set([...english.keys()].map(baseKey));
  const translatedBases = new Set([...translated.keys()].map(baseKey));
  for (const base of englishBases) {
    if (!translatedBases.has(base)) problems.push(`${locale}/${namespace}: missing ${base}`);
  }
  for (const base of translatedBases) {
    if (!englishBases.has(base)) problems.push(`${locale}/${namespace}: extra ${base}`);
  }
  for (const [key, text] of translated) {
    if (text.trim() === "") problems.push(`${locale}/${namespace}: empty ${key}`);
  }
  const englishPlaceholders = tokensByBase(english, PLACEHOLDER);
  const translatedPlaceholders = tokensByBase(translated, PLACEHOLDER);
  const englishTags = tokensByBase(english, TAG);
  const translatedTags = tokensByBase(translated, TAG);
  for (const base of englishBases) {
    if (!translatedBases.has(base)) continue;
    if (!sameSet(englishPlaceholders.get(base)!, translatedPlaceholders.get(base)!)) {
      problems.push(`${locale}/${namespace}: placeholders differ in ${base}`);
    }
    if (!sameSet(englishTags.get(base)!, translatedTags.get(base)!)) {
      problems.push(`${locale}/${namespace}: markup tags differ in ${base}`);
    }
    if (locale === "pl" && pluralForms(english, base).length > 0) {
      const forms = pluralForms(translated, base);
      const missing = POLISH_PLURAL_FORMS.filter((form) => !forms.includes(form));
      if (missing.length > 0) {
        problems.push(`${locale}/${namespace}: ${base} lacks plural forms ${missing.join(", ")}`);
      }
    }
  }
  return problems;
}

export function findCatalogProblems(resources: Resource, sourceLocale = "en"): string[] {
  const source = resources[sourceLocale] ?? {};
  const problems: string[] = [];
  for (const [locale, namespaces] of Object.entries(resources)) {
    if (locale === sourceLocale) continue;
    const allNamespaces = new Set([...Object.keys(source), ...Object.keys(namespaces)]);
    for (const namespace of allNamespaces) {
      if (!(namespace in namespaces)) {
        problems.push(`${locale}: missing namespace ${namespace}`);
        continue;
      }
      if (!(namespace in source)) {
        problems.push(`${locale}: namespace ${namespace} has no ${sourceLocale} source`);
        continue;
      }
      const english = flatten(source[namespace], "", new Map());
      const translated = flatten(namespaces[namespace], "", new Map());
      problems.push(...compareNamespace(namespace, english, translated, locale));
    }
  }
  return problems;
}
