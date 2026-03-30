import { EnhancedProjectDetector } from '../core/project-detector.js';
import { CLAUDEMdMaintainer } from '../core/claude-md-maintainer.js';
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
