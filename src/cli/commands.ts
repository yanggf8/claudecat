import { createInterface } from 'readline';
import { EnhancedProjectDetector } from '../core/project-detector.js';
import { CLAUDEMdMaintainer } from '../core/claude-md-maintainer.js';
import { TursoClient } from '../cloud/turso-client.js';
import { ConfigStore } from '../cloud/config-store.js';
import { ProjectIdentifier } from '../cloud/project-identifier.js';
import { getMachineId } from '../cloud/machine-id.js';
import { formatScanResult, formatStatus } from './formatter.js';
import type { ParsedArgs } from './args.js';

export async function runScan(projectRoot: string, flags: ParsedArgs['flags']): Promise<void> {
  const detector = new EnhancedProjectDetector(projectRoot);
  const context = detector.detectCurrentContext();

  if (flags.json) {
    process.stdout.write(JSON.stringify(context, null, 2) + '\n');
  } else {
    console.log(formatScanResult(context));
  }
}

export async function runUpdate(projectRoot: string, flags: ParsedArgs['flags']): Promise<void> {
  const detector = new EnhancedProjectDetector(projectRoot);
  const context = detector.detectCurrentContext();
  const maintainer = new CLAUDEMdMaintainer(projectRoot);

  await maintainer.updateProjectContext(context);

  if (flags.json) {
    process.stdout.write(JSON.stringify({ updated: true, context }, null, 2) + '\n');
  }
}

export async function runStatus(projectRoot: string, flags: ParsedArgs['flags']): Promise<void> {
  const detector = new EnhancedProjectDetector(projectRoot);
  const context = detector.detectCurrentContext();

  if (flags.json) {
    process.stdout.write(JSON.stringify(context, null, 2) + '\n');
  } else {
    console.log(formatStatus(context));
  }
}

function prompt(question: string): Promise<string> {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  return new Promise((resolve) => {
    rl.question(question, (answer) => {
      rl.close();
      resolve(answer.trim());
    });
  });
}

export async function runLogin(options: ParsedArgs['options']): Promise<void> {
  let url = options.url;
  let token = options.token;

  if (!url) url = await prompt('Turso database URL: ');
  if (!token) token = await prompt('Turso auth token: ');

  if (!url || !token) {
    console.error('Both URL and token are required.');
    process.exitCode = 1;
    return;
  }

  // Validate connection
  const client = new TursoClient(url, token);
  try {
    await client.migrate();
    console.log('Connection verified. Schema ready.');
  } catch (error) {
    console.error(`Authentication failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
    return;
  } finally {
    client.close();
  }

  ConfigStore.write({ tursoUrl: url, tursoToken: token });
  console.log('Credentials saved to ~/.claudecat/config.json');
}

export async function runSync(projectRoot: string, flags: ParsedArgs['flags']): Promise<void> {
  const config = ConfigStore.read();
  if (!config) {
    console.error('Not configured. Run: claudecat login --url <turso-url> --token <token>');
    process.exitCode = 1;
    return;
  }

  const client = new TursoClient(config.tursoUrl, config.tursoToken);
  try {
    await client.migrate();

    const { id, displayName } = ProjectIdentifier.resolve(projectRoot);
    const machineId = getMachineId();

    // Scan current project
    const detector = new EnhancedProjectDetector(projectRoot);
    const context = detector.detectCurrentContext();

    // Pull first — check if remote has newer patterns before pushing
    const remote = await client.getLatestSnapshot(id);
    const remoteIsNewer = remote
      && remote.machineId !== machineId
      && remote.scannedAt > context.lastUpdated;

    if (remoteIsNewer && !flags.force) {
      // Remote has newer patterns from another machine — apply them locally
      const maintainer = new CLAUDEMdMaintainer(projectRoot);
      await maintainer.updateProjectContext(remote.context);

      if (flags.json) {
        process.stdout.write(JSON.stringify({ action: 'pulled', from: remote.machineId, context: remote.context }, null, 2) + '\n');
      } else {
        console.log(`Pulled newer patterns from machine ${remote.machineId}`);
      }
    } else {
      // Push local patterns (local is newer or force flag set)
      await client.upsertProject(id, displayName);
      await client.pushSnapshot(id, machineId, context);

      if (flags.json) {
        process.stdout.write(JSON.stringify({ action: 'pushed', project: id, machine: machineId }, null, 2) + '\n');
      } else {
        console.log(`Synced: ${displayName} (${id})`);
      }
    }
  } catch (error) {
    console.error(`Sync failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  } finally {
    client.close();
  }
}
