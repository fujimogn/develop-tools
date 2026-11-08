import { expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const harness =
  process.env.HEAVY_CHECK_TEST_HARNESS ??
  resolve(import.meta.dir, '../target/debug/examples/heavy-check-harness')
const fixtureBase = resolve(import.meta.dir, '../target/heavy-check-deadline-fixtures')
const budgetMs = 1_000

function groupExists(group: number): boolean {
  try {
    process.kill(-group, 0)
    return true
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ESRCH') return false
    throw error
  }
}

function killGroup(group: number): void {
  try {
    process.kill(-group, 'SIGKILL')
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error
  }
}

async function eventually(check: () => boolean, description: string): Promise<void> {
  const limit = performance.now() + 5_000
  while (!check()) {
    if (performance.now() >= limit) throw new Error(description)
    await Bun.sleep(10)
  }
}

function start(args: string[]) {
  // Bun inherits only the configured stdio descriptors. Remove the matching
  // capability labels too; fixture admission must never consume an outer slot.
  const env = { ...process.env }
  for (const key of [
    'DEVELOP_HEAVY_CHECK_SLOT_FD',
    'DEVELOP_HEAVY_CHECK_CONTROL_FD',
    'DEVELOP_HEAVY_CHECK_EVENT_FD',
    'DEVELOP_HEAVY_CHECK_RESULT_FD',
  ]) Reflect.deleteProperty(env, key)
  const child = Bun.spawn([harness, ...args], {
    env,
    stdin: 'pipe',
    stdout: 'pipe',
    stderr: 'inherit',
  })
  const lines: string[] = []
  const reading = (async () => {
    let partial = ''
    const decoder = new TextDecoder()
    for await (const chunk of child.stdout) {
      partial += decoder.decode(chunk, { stream: true })
      while (partial.includes('\n')) {
        const newline = partial.indexOf('\n')
        lines.push(partial.slice(0, newline))
        partial = partial.slice(newline + 1)
      }
    }
    if (partial) lines.push(partial)
  })()
  return { child, lines, reading }
}

async function deadlineFixture(beforeAdmission: boolean) {
  await mkdir(fixtureBase, { recursive: true, mode: 0o700 })
  const root = await mkdtemp(join(fixtureBase, 'develop-meta-deadline-'))
  const home = join(root, 'account')
  await mkdir(home, { mode: 0o700 })
  let holder: ReturnType<typeof start> | undefined
  let runner: ReturnType<typeof start> | undefined
  let group: number | undefined
  let stopped = false
  let timer: ReturnType<typeof setTimeout> | undefined
  let safety: ReturnType<typeof setTimeout> | undefined
  let heartbeat: ReturnType<typeof setInterval> | undefined
  try {
    if (beforeAdmission) {
      holder = start([home, '.cache/develop-tools/heavy-check', '0', '--queue'])
      await eventually(() => holder?.lines.includes('acquired') ?? false, 'holder never acquired')
    }
    const started = performance.now()
    const deadline = started + budgetMs
    runner = start(['--family', home, 'ignore-term'])
    let firedAt = 0
    let heartbeatCount = 0
    let safetyFired = false
    const stop = () => {
      firedAt = performance.now()
      if (group !== undefined) killGroup(group)
    }
    const armDeadline = () => {
      timer = setTimeout(stop, Math.max(0, deadline - performance.now()))
    }
    armDeadline()
    // Monitoring heartbeats report progress; they never move the wall deadline.
    heartbeat = setInterval(() => {
      heartbeatCount += 1
    }, 40)
    safety = setTimeout(() => {
      safetyFired = true
      stop()
    }, budgetMs * 4)
    await eventually(() => {
      const line = runner?.lines.find(value => value.startsWith('group:'))
      if (!line) return false
      group = Number(line.slice('group:'.length))
      return Number.isInteger(group) && group > 1
    }, 'supervisor never announced a family group')
    await eventually(
      () => runner?.lines.some(line => line.startsWith('queued:')) ?? false,
      'worker never queued',
    )
    if (!beforeAdmission) {
      await eventually(() => runner?.lines.includes('acquired') ?? false, 'worker never acquired')
    }
    await eventually(
      () => runner?.child.exitCode !== null,
      'supervisor did not stop after deadline',
    )
    await runner.child.exited
    await runner.reading
    const familyGroup = group
    if (familyGroup === undefined) throw new Error('missing family group')
    await eventually(() => !groupExists(familyGroup), 'family group survived deadline')
    stopped = true
    const acquired = runner.lines.includes('acquired')
    const raw = runner.lines.find(line => line.startsWith('worker-exit:'))
    expect(raw).toBe('worker-exit:137')
    expect(acquired).toBe(!beforeAdmission)
    expect(acquired ? 'executed-timeout' : 'unexecuted').toBe(
      beforeAdmission ? 'unexecuted' : 'executed-timeout',
    )
    expect(heartbeatCount).toBeGreaterThan(5)
    expect(safetyFired).toBe(false)
    expect(firedAt - started).toBeGreaterThanOrEqual(budgetMs - 20)
    expect(firedAt - started).toBeLessThan(budgetMs + 700)
  } finally {
    clearTimeout(timer)
    clearTimeout(safety)
    clearInterval(heartbeat)
    if (group !== undefined && !stopped) {
      const familyGroup = group
      killGroup(familyGroup)
      await eventually(() => runner?.child.exitCode !== null, 'cleanup refused: supervisor remains')
      await runner?.child.exited
      await eventually(() => !groupExists(familyGroup), 'cleanup refused: group still exists')
      stopped = true
    }
    if (runner && group === undefined) {
      runner.child.kill('SIGKILL')
      await runner.child.exited
    }
    if (holder) {
      holder.child.stdin.write('release\n')
      holder.child.stdin.end()
      await holder.child.exited
      await holder.reading
    }
    // Removing state is authorized only after the full group was confirmed stopped.
    if (!runner || stopped || group === undefined) await rm(root, { recursive: true })
  }
}

test('outer deadline before admission keeps raw137 separate from unexecuted classification', async () => {
  await deadlineFixture(true)
}, 15_000)

test('outer deadline after admission is unchanged by monitoring heartbeats', async () => {
  await deadlineFixture(false)
}, 15_000)
