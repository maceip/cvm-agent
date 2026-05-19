# Runcard Card Lab

This directory is a design sandbox for the LLM Attested run cards.

The split is deliberate:

- Team cards are for hackathon judging. They should make team membership,
  capture coverage, challenge score, token source, and assurance mix easy to
  compare.
- Individual cards are for daily proof culture. They should make a single
  builder's active capture path, attestation posture, latest event, and public
  usage stats easy to share.

## Concepts

Team concepts:

- `team-ledger.svg`: public competition scorecard with rank, totals, capture
  mix, and assurance distribution.
- `team-relay.svg`: proof graph showing multiple participants feeding one team
  card.
- `team-scoreline.svg`: challenge-first card for minimum-token or maximum-token
  competitions.
- `team-terminal.svg`: compact developer-native status card for READMEs and CI.

Individual concepts:

- `individual-passport.svg`: personal proof passport with identity, device
  proof, and current capture posture.
- `individual-signal.svg`: live status card with active path, latest event, and
  actionable capture health.
- `individual-tokenstream.svg`: public token-use brag card with model mix and
  visibility labels.
- `individual-pocket.svg`: high-contrast compact card for social profiles.

## Renderer Vocabulary

The point of the lab is not to create eight unrelated templates. Every concept
is trying to fit the same narrow renderer vocabulary:

- `subject_type`: team or individual
- `subject_name`
- `event_name` or universal profile scope
- `capture_method`
- `assurance`
- `enforcement`
- `token_source`
- `latest_event`
- `stats`
- `visibility_policy`
- `receipt_url`

That keeps the eventual production renderer small while leaving room for skins,
themes, and organizer-specific competition modes.

## Live Individual Signal

The hosted event service now exposes the Individual Signal concept as a plain
SVG suitable for a GitHub profile README:

```md
![Runcard signal](https://events.example/e/<event_id>/teams/<team_id>/runcard.signal.svg)
```

The companion JSON is available at the same path with `.json`:

```http
GET /e/<event_id>/teams/<team_id>/runcard.signal.json
GET /e/<event_id>/teams/<team_id>/runcard.signal.svg
```

For multi-agent teams, pass `?agent=<agent_session_id>` to pin the card to one
participant/agent session. Without that query, the signal card follows the
latest active participant for the team, which works well for a one-person
universal profile.

The SVG itself also carries a copy of the card document in a `<metadata>` block:

```xml
<metadata
  id="runcard-card-json"
  data-content-type="application/json"
  data-sha256="sha256:...">
  {"profile":"https://runcard.dev/llm-individual-signal/v1", ...}
</metadata>
```

That is not the trust anchor by itself; the JSON/proof/receipt endpoints remain
canonical. The embedded metadata makes screenshots, caches, and copied SVGs
self-describing enough for tools and curious humans to find the proof trail.

Regenerate the assets with:

```bash
node docs/card-lab/scripts/generate-card-concepts.mjs
```
