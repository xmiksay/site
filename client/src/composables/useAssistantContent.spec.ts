import { describe, it, expect } from 'vitest'
import { decisionFor, needsDecision, toolCalls } from './useAssistantContent'

// `needsDecision` exists to fix a real bug: a batch can mix a call that's
// genuinely still awaiting approval with one the policy auto-allowed (never
// gets a `decisions` entry, per `src/ai/projection/mod.rs`'s `OpenTurn`) or
// whose own decision landed on a different projected message than this one
// (an engine/projection timing quirk — see that file's `mark_resolved_calls`
// doc). The old check (`requiresApproval(content) && decisionFor(...) ===
// undefined`) couldn't tell these apart from a genuinely-pending call, so it
// kept offering a dead Allow/Reject prompt for something already resolved —
// "a historic tool prompt looks unresolved, but it's actually done".
describe('toolCalls', () => {
  it('maps requires_approval and resolved from the raw projected content', () => {
    const content = {
      tool_calls: [
        { id: 'a', name: 'page_edit', args: {}, requires_approval: true, resolved: true },
        { id: 'b', name: 'page_search', args: {} },
      ],
    }
    const calls = toolCalls(content)
    expect(calls[0]).toMatchObject({ id: 'a', requiresApproval: true, resolved: true })
    expect(calls[1]).toMatchObject({ id: 'b', requiresApproval: false, resolved: false })
  })
})

describe('needsDecision', () => {
  it('is true only for a call still gated and not yet resolved', () => {
    expect(needsDecision({ id: 'a', name: 'page_edit', args: {}, requiresApproval: true, resolved: false })).toBe(
      true,
    )
  })

  it('is false for an auto-allowed call that never needed approval at all', () => {
    // `content.requires_approval` (the message-level flag) can be true
    // because a *sibling* call in the batch needs approval — this call's own
    // `requires_approval` stays false, so it must never show a prompt.
    expect(needsDecision({ id: 'b', name: 'page_search', args: {}, requiresApproval: false, resolved: false })).toBe(
      false,
    )
  })

  it('is false once a gated call has resolved, even without a matching decisions entry', () => {
    // The orphaned-decision case: the call genuinely needed approval and has
    // already run (a `tool_result` exists for it — `resolved: true`), but its
    // own `InMsg::Approve`/`Reject` record was folded onto a different,
    // already-flushed message. `decisionFor` would still say "undecided" for
    // it; `needsDecision` must not.
    const content = { decisions: [] }
    expect(decisionFor(content, 'c')).toBeUndefined()
    expect(needsDecision({ id: 'c', name: 'page_edit', args: {}, requiresApproval: true, resolved: true })).toBe(
      false,
    )
  })
})
