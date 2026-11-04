import { useEffect, useState } from 'react'

/**
 * A page's own choices, kept past the page. Every tab is unmounted when another is opened, so
 * plain component state forgot its sort, filters and search the moment the player looked away.
 *
 * Sorts, filters and toggles are kept in `localStorage` and come back after a restart, the way the
 * price floor does. What was typed into a search box and which page of results was open are kept in
 * `sessionStorage`, which lasts as long as the window: coming back to a tab should find it as it
 * was left, but a search from yesterday silently hiding most of the collection would not be.
 */
export type Lifetime = 'restart' | 'session'

const PREFIX = 'tennoscope.view.'

function storage(lifetime: Lifetime): Storage {
  return lifetime === 'restart' ? localStorage : sessionStorage
}

/** Storage is allowed to be missing or refused, and a value from an older build may name a choice
 * that no longer exists: either way the page starts from its default. */
function recall<T>(key: string, initial: T, accept: (value: unknown) => boolean, lifetime: Lifetime): T {
  try {
    const raw = storage(lifetime).getItem(PREFIX + key)
    if (raw === null) return initial
    const value: unknown = JSON.parse(raw)
    return accept(value) ? value as T : initial
  } catch {
    return initial
  }
}

export function useRemembered<T>(key: string, initial: T, accept: (value: unknown) => boolean, lifetime: Lifetime = 'restart') {
  const [value, setValue] = useState<T>(() => recall(key, initial, accept, lifetime))
  useEffect(() => {
    try {
      storage(lifetime).setItem(PREFIX + key, JSON.stringify(value))
    } catch {
      // A choice that cannot be saved is still the choice on screen.
    }
  }, [key, value, lifetime])
  return [value, setValue] as const
}

/** Accepts exactly the listed values. */
export function oneOf<T>(values: readonly T[]): (value: unknown) => boolean {
  return value => values.includes(value as T)
}

export const isString = (value: unknown) => typeof value === 'string'
export const isBoolean = (value: unknown) => typeof value === 'boolean'
export const isPageNumber = (value: unknown) => Number.isInteger(value) && (value as number) >= 1
