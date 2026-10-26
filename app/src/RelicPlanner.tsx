import { Fragment, useEffect, useMemo, useState } from 'react'
import { MetalMark } from './MetalMark'
import { getRelicPlan, type AccessMode, type PlanCurrency, type Refinement, type RelicPlanEntry, type RelicPlanRow, type RelicPlanView } from './backend'

const REFINEMENTS: Refinement[] = ['intact', 'exceptional', 'flawless', 'radiant']
const refinementLabel: Record<Refinement, string> = { intact: 'Intact', exceptional: 'Exceptional', flawless: 'Flawless', radiant: 'Radiant' }
/** How long to wait before asking again while the drop tables load. The first load parses the item catalogue, which takes seconds. */
const LOADING_RETRY_MS = 1500

const ERAS = ['Lith', 'Meso', 'Neo', 'Axi', 'Requiem'] as const
type Era = typeof ERAS[number]
/** A relic's era is the first word of its name, as in "Axi A1" or "Requiem IV". */
const eraOf = (relic: string) => relic.slice(0, relic.indexOf(' '))

type SortKey = 'gain' | 'value' | 'radiant'
interface Sort { key: SortKey; descending: boolean }

function figure(value: number): string {
  return value.toFixed(1)
}

function percent(chance: number): string {
  return `${(chance * 100).toFixed(chance < 0.1 ? 1 : 0)}%`
}

function Value({ value, currency, signed }: { value: number; currency: PlanCurrency; signed?: boolean }) {
  const text = signed && value > 0 ? `+${figure(value)}` : figure(value)
  return <span className={`relic-value${signed && value < 0 ? ' relic-loss' : ''}`}>{text}<MetalMark metal={currency === 'platinum' ? 'plat' : 'ducat'} alt={currency === 'platinum' ? ' platinum' : ' ducats'}/></span>
}

const entryKey = (entry: RelicPlanEntry) => `${entry.relic}-${entry.refinement}`

/** Compares two entries by one column, larger first. A Radiant entry has no gain and sorts last either way. */
function compare(left: RelicPlanEntry, right: RelicPlanEntry, sort: Sort): number {
  if (sort.key === 'gain') {
    if (left.gain_per_100 === null || right.gain_per_100 === null) return Number(left.gain_per_100 === null) - Number(right.gain_per_100 === null)
    const order = right.gain_per_100 - left.gain_per_100
    return sort.descending ? order : -order
  }
  const pick = (entry: RelicPlanEntry) => sort.key === 'value' ? entry.value : entry.radiant_value
  const order = pick(right) - pick(left)
  return sort.descending ? order : -order
}

export function RelicPlanner({ effectiveMode }: { effectiveMode: AccessMode }) {
  const [currency, setCurrency] = useState<PlanCurrency>('platinum')
  const [squad, setSquad] = useState(1)
  const [search, setSearch] = useState('')
  const [onlyOwned, setOnlyOwned] = useState(true)
  const [era, setEra] = useState<Era | null>(null)
  const [sort, setSort] = useState<Sort>({ key: 'gain', descending: true })
  const [expanded, setExpanded] = useState<string | null>(null)
  const [view, setView] = useState<RelicPlanView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)

  useEffect(() => {
    let active = true
    let retry: ReturnType<typeof setTimeout> | undefined
    getRelicPlan({ currency, squad })
      .then(next => {
        if (!active) return
        setView(next)
        setError(null)
        if (next.status === 'loading') retry = setTimeout(() => setAttempt(value => value + 1), LOADING_RETRY_MS)
      })
      .catch(() => { if (active) setError('The relic planner could not be reached.') })
    return () => {
      active = false
      if (retry) clearTimeout(retry)
    }
  }, [currency, squad, attempt])

  const plan = view?.plan ?? null
  const rowsByName = useMemo(() => new Map((plan?.rows ?? []).map(row => [row.name, row])), [plan])
  const needle = search.trim().toLowerCase()
  /** Relics whose name or any reward's name holds the search, so "citrine" finds every relic with a Citrine Prime part. */
  const matching = useMemo(() => new Set((plan?.rows ?? [])
    .filter(row => row.name.toLowerCase().includes(needle) || row.rewards.some(reward => reward.name.toLowerCase().includes(needle)))
    .map(row => row.name)), [plan, needle])

  const entries = useMemo(() => {
    const shown = (plan?.entries ?? []).filter(entry => (!needle || matching.has(entry.relic) || entry.relic.toLowerCase().includes(needle)) && (!onlyOwned || entry.copies > 0) && (!era || eraOf(entry.relic) === era))
    return [...shown].sort((left, right) => compare(left, right, sort) || left.relic.localeCompare(right.relic) || REFINEMENTS.indexOf(left.refinement) - REFINEMENTS.indexOf(right.refinement))
  }, [plan, needle, matching, onlyOwned, era, sort])

  const sortBy = (key: SortKey) => setSort(current => current.key === key ? { key, descending: !current.descending } : { key, descending: true })
  const header = (key: SortKey, label: string) => <th aria-sort={sort.key === key ? (sort.descending ? 'descending' : 'ascending') : undefined}>
    <button type="button" className="relic-sort" onClick={() => sortBy(key)}>{label}{sort.key === key ? (sort.descending ? ' ↓' : ' ↑') : ''}</button>
  </th>

  return <section className="page" aria-labelledby="relics-title">
    <div className="mark-head">
      <h1 id="relics-title" className="mark">Relic planner</h1>
      <p className="prose">What each relic is worth opened as it is and at Radiant, at your squad size. Gain per 100 traces puts every refinement on the same footing: high is where Void Traces buy the most, and at or below zero a relic is best opened as it is.</p>
    </div>

    <div className="register">
      <div className="register-controls">
        <div className="sort-slot" role="group" aria-label="Value relics in">
          <span>Value</span>
          <div className="tally">
            <button type="button" aria-pressed={currency === 'platinum'} onClick={() => setCurrency('platinum')}><MetalMark metal="plat"/>Platinum</button>
            <button type="button" aria-pressed={currency === 'ducats'} onClick={() => setCurrency('ducats')}><MetalMark metal="ducat"/>Ducats</button>
          </div>
        </div>
        <div className="sort-slot" role="group" aria-label="Squad size">
          <span>Squad</span>
          <div className="tally">
            {[1, 2, 3, 4].map(size => <button type="button" key={size} aria-pressed={squad === size} onClick={() => setSquad(size)}>{size}</button>)}
          </div>
        </div>
        <div className="sort-slot" role="group" aria-label="Relic era">
          <span>Era</span>
          <div className="tally">
            <button type="button" aria-pressed={era === null} onClick={() => setEra(null)}>Any</button>
            {ERAS.map(name => <button type="button" key={name} aria-pressed={era === name} onClick={() => setEra(name)}>{name}</button>)}
          </div>
        </div>
        <label className="relic-check"><input type="checkbox" checked={onlyOwned} onChange={event => setOnlyOwned(event.target.checked)}/>Only owned</label>
        <label className="search-slot">
          <svg className="punch-glyph" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="square" strokeLinejoin="miter"><circle cx="10.5" cy="10.5" r="7"/><path d="M15.5 15.5 22 22"/></svg>
          <span className="sr-only">Search relics or rewards</span>
          <input type="search" aria-label="Search relics or rewards" placeholder="Search relics or rewards…" value={search} onChange={event => setSearch(event.target.value)}/>
        </label>
      </div>

      {error && <p className="error-banner" role="alert">{error}</p>}
      {!view || view.status === 'loading'
        ? <p className="register-line" role="status">Loading relic drop tables…</p>
        : view.status === 'unavailable' || !plan
          ? <p className="register-line" role="status">The relic drop tables could not be loaded. Check the connection, then <button type="button" className="text-action" onClick={() => setAttempt(value => value + 1)}>try again</button>.</p>
          : <>
            {!view.inventory_known && <p className="register-line">No inventory has been saved yet, so owned counts are empty.{effectiveMode !== 'full' && ' Full access reads them from the game.'} Untick Only owned to see every relic.</p>}
            <table className="relic-table" aria-label="Relics">
              <thead><tr>
                <th>Relic</th>
                <th>Refinement</th>
                <th>Owned</th>
                {header('value', 'Value now')}
                {header('radiant', 'Value at Radiant')}
                {header('gain', 'Gain per 100 traces')}
              </tr></thead>
              <tbody>
                {entries.map(entry => {
                  const key = entryKey(entry)
                  const row = rowsByName.get(entry.relic)
                  return <Fragment key={key}>
                    <tr className="relic-row" onClick={() => setExpanded(current => current === key ? null : key)}>
                      <th scope="row"><RelicName name={entry.relic} vaulted={entry.vaulted} expanded={expanded === key}/></th>
                      <td>{refinementLabel[entry.refinement]}</td>
                      <td>{entry.copies || '–'}</td>
                      <td><Value value={entry.value} currency={currency}/></td>
                      <td><Value value={entry.radiant_value} currency={currency}/></td>
                      <td>{entry.gain_per_100 === null ? '–' : <Value value={entry.gain_per_100} currency={currency} signed/>}</td>
                    </tr>
                    {expanded === key && row && <tr className="relic-detail"><td colSpan={6}><RewardTable row={row} currency={currency} refinement={entry.refinement}/></td></tr>}
                  </Fragment>
                })}
              </tbody>
            </table>
            {!entries.length && <p className="register-line">No relics match.</p>}
          </>}
    </div>
  </section>
}

function RelicName({ name, vaulted, expanded }: { name: string; vaulted: boolean; expanded: boolean }) {
  // The row toggles on click; this button is the same toggle reached by keyboard.
  return <button type="button" className="relic-name" aria-expanded={expanded}>{name}{vaulted && <em className="relic-vaulted">Vaulted</em>}</button>
}

/** The rewards with their chance at the held refinement and at Radiant. */
function RewardTable({ row, currency, refinement }: { row: RelicPlanRow; currency: PlanCurrency; refinement: Refinement }) {
  const shown: Refinement[] = refinement === 'radiant' ? ['radiant'] : [refinement, 'radiant']
  return <table className="relic-rewards" aria-label={`${row.name} rewards`}>
    <thead><tr><th>Reward</th>{shown.map(step => <th key={step}>{refinementLabel[step]}</th>)}<th>Platinum</th><th>Ducats</th><th>Owned</th></tr></thead>
    <tbody>
      {[...row.rewards].sort((left, right) => right.chances[0] - left.chances[0]).map((reward, index) => <tr key={`${reward.name}-${index}`}>
        <th scope="row">{reward.name}</th>
        {shown.map(step => <td key={step}>{percent(reward.chances[REFINEMENTS.indexOf(step)])}</td>)}
        <td>{reward.platinum ?? '–'}</td>
        <td>{reward.ducats ?? '–'}</td>
        <td>{reward.owned}</td>
      </tr>)}
    </tbody>
    {row.unvalued > 0 && <caption>
      {`${row.unvalued} ${row.unvalued === 1 ? 'reward has' : 'rewards have'} no ${currency === 'platinum' ? 'price' : 'ducat value'}, counted as zero.`}
    </caption>}
  </table>
}
