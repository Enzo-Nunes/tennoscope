import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { RelicPlanView } from './backend'

const backend = vi.hoisted(() => ({ getRelicPlan: vi.fn() }))
vi.mock('./backend', () => backend)

import { RelicPlanner } from './RelicPlanner'

const chances = {
  common: [0.2533, 0.2333, 0.2, 0.1667],
  rare: [0.02, 0.04, 0.06, 0.1],
}

const ready: RelicPlanView = {
  status: 'ready',
  inventory_known: true,
  plan: {
    currency: 'platinum',
    squad: 1,
    rows: [
      {
        name: 'Axi A1', vaulted: true, owned: [3, 0, 1, 1], value: [10.67, 12.27, 14.4, 17.33], unvalued: 1,
        rewards: [
          { name: 'Braton Prime Stock', chances: chances.common, platinum: 5, ducats: 15, owned: 2 },
          { name: 'Nikana Prime Blueprint', chances: chances.rare, platinum: 60, ducats: 100, owned: 0 },
        ],
      },
      { name: 'Lith B1', vaulted: false, owned: [2, 0, 0, 0], value: [29.92, 29.8, 29.36, 28.44], unvalued: 0, rewards: [] },
      { name: 'Meso C1', vaulted: false, owned: [0, 0, 0, 0], value: [8, 9, 10, 12], unvalued: 0, rewards: [] },
    ],
    entries: [
      { relic: 'Axi A1', vaulted: true, refinement: 'intact', copies: 3, value: 10.67, radiant_value: 17.33, gain_per_100: 6.66 },
      { relic: 'Axi A1', vaulted: true, refinement: 'flawless', copies: 1, value: 14.4, radiant_value: 17.33, gain_per_100: 5.86 },
      { relic: 'Axi A1', vaulted: true, refinement: 'radiant', copies: 1, value: 17.33, radiant_value: 17.33, gain_per_100: null },
      { relic: 'Lith B1', vaulted: false, refinement: 'intact', copies: 2, value: 29.92, radiant_value: 28.44, gain_per_100: -1.48 },
      { relic: 'Meso C1', vaulted: false, refinement: 'intact', copies: 0, value: 8, radiant_value: 12, gain_per_100: 4 },
    ],
  },
}

const relicCells = () => within(screen.getByRole('table', { name: 'Relics' })).getAllByRole('row').slice(1)
  .filter(row => !row.classList.contains('relic-detail'))
  .map(row => [row.querySelector('th')?.textContent, row.querySelectorAll('td')[0]?.textContent].join(' '))

beforeEach(() => {
  backend.getRelicPlan.mockReset()
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('RelicPlanner', () => {
  it('lists each held refinement, best use of traces first, with Radiant last', async () => {
    backend.getRelicPlan.mockResolvedValue(ready)
    render(<RelicPlanner effectiveMode="full"/>)
    await screen.findByRole('table', { name: 'Relics' })

    expect(relicCells()).toEqual([
      'Axi A1Vaulted Intact',
      'Axi A1Vaulted Flawless',
      'Lith B1 Intact',
      'Axi A1Vaulted Radiant',
    ])
    const row = within(screen.getByRole('table', { name: 'Relics' })).getByRole('row', { name: /Lith B1/ })
    expect(row).toHaveTextContent('29.9')
    expect(row).toHaveTextContent('28.4')
    expect(row).toHaveTextContent('-1.5')
  })

  it('reverses the gain order so relics best opened as they are come first', async () => {
    backend.getRelicPlan.mockResolvedValue(ready)
    render(<RelicPlanner effectiveMode="full"/>)
    await screen.findByRole('table', { name: 'Relics' })

    fireEvent.click(screen.getByRole('button', { name: /Gain per 100 traces/ }))
    expect(relicCells()).toEqual([
      'Lith B1 Intact',
      'Axi A1Vaulted Flawless',
      'Axi A1Vaulted Intact',
      'Axi A1Vaulted Radiant',
    ])
    expect(screen.getByRole('columnheader', { name: /Gain per 100 traces/ })).toHaveAttribute('aria-sort', 'ascending')
  })

  it('ranks by the value of opening each copy as it is', async () => {
    backend.getRelicPlan.mockResolvedValue(ready)
    render(<RelicPlanner effectiveMode="full"/>)
    await screen.findByRole('table', { name: 'Relics' })

    fireEvent.click(screen.getByRole('button', { name: /Value now/ }))
    expect(relicCells()).toEqual([
      'Lith B1 Intact',
      'Axi A1Vaulted Radiant',
      'Axi A1Vaulted Flawless',
      'Axi A1Vaulted Intact',
    ])
    expect(screen.queryByRole('button', { name: /^Relic/ })).toBeNull()
  })

  it('ranks by the value of opening each copy at Radiant', async () => {
    backend.getRelicPlan.mockResolvedValue(ready)
    render(<RelicPlanner effectiveMode="full"/>)
    await screen.findByRole('table', { name: 'Relics' })

    fireEvent.click(screen.getByRole('button', { name: /Value at Radiant/ }))
    expect(relicCells()).toEqual([
      'Lith B1 Intact',
      'Axi A1Vaulted Intact',
      'Axi A1Vaulted Flawless',
      'Axi A1Vaulted Radiant',
    ])
    expect(screen.getByRole('columnheader', { name: /Value at Radiant/ })).toHaveAttribute('aria-sort', 'descending')
  })

  it('shows relics not held, at Intact, when only owned is unticked', async () => {
    backend.getRelicPlan.mockResolvedValue(ready)
    render(<RelicPlanner effectiveMode="full"/>)
    await screen.findByRole('table', { name: 'Relics' })

    fireEvent.click(screen.getByRole('checkbox', { name: 'Only owned' }))
    expect(relicCells()).toContain('Meso C1 Intact')
  })

  it('asks again with the squad and currency the player picks', async () => {
    backend.getRelicPlan.mockResolvedValue(ready)
    render(<RelicPlanner effectiveMode="full"/>)
    await screen.findByRole('table', { name: 'Relics' })

    fireEvent.click(within(screen.getByRole('group', { name: 'Squad size' })).getByRole('button', { name: '4' }))
    fireEvent.click(screen.getByRole('button', { name: 'Ducats' }))
    await act(async () => {})
    expect(backend.getRelicPlan).toHaveBeenLastCalledWith({ currency: 'ducats', squad: 4 })
  })

  it('opens a row into its rewards at that refinement and at Radiant', async () => {
    backend.getRelicPlan.mockResolvedValue(ready)
    render(<RelicPlanner effectiveMode="full"/>)
    const table = await screen.findByRole('table', { name: 'Relics' })

    fireEvent.click(within(table).getAllByRole('row', { name: /Axi A1/ })[0])
    const rewards = screen.getByRole('table', { name: 'Axi A1 rewards' })
    const headers = within(rewards).getAllByRole('columnheader').map(cell => cell.textContent)
    expect(headers.slice(0, 3)).toEqual(['Reward', 'Intact', 'Radiant'])
    expect(within(rewards).getByRole('row', { name: /Nikana Prime Blueprint/ })).toHaveTextContent('2.0%')
    expect(rewards.querySelector('caption')).toHaveTextContent('1 reward has no price, counted as zero.')
  })

  it('keeps asking while the drop tables load', async () => {
    vi.useFakeTimers()
    backend.getRelicPlan.mockResolvedValueOnce({ status: 'loading', plan: null, inventory_known: false }).mockResolvedValue(ready)
    render(<RelicPlanner effectiveMode="companion"/>)
    await act(async () => {})
    expect(screen.getByText('Loading relic drop tables…')).toBeInTheDocument()

    await act(async () => { await vi.advanceTimersByTimeAsync(1500) })
    expect(screen.getByRole('table', { name: 'Relics' })).toBeInTheDocument()
  })

  it('says when no inventory is saved', async () => {
    backend.getRelicPlan.mockResolvedValue({ ...ready, inventory_known: false })
    render(<RelicPlanner effectiveMode="companion"/>)
    expect(await screen.findByText(/No inventory has been saved yet/)).toHaveTextContent('Full access reads them from the game.')
  })
})
