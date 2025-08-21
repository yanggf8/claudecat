import { glob } from 'glob';
import { readFileSync, existsSync, statSync } from 'fs';
import { join, dirname } from 'path';
import type { 
  ProjectContextInfo, 
  ImplementationPatterns, 
  PatternDetectionSignal,
  ConfidenceCalculation 
} from '../types/patterns.js';

export class EnhancedProjectDetector {
  private projectRoot: string;

  constructor(projectRoot: string = process.cwd()) {
    this.projectRoot = projectRoot;
  }

  /**
   * Main detection method - analyzes project and returns comprehensive context
   */
  detectCurrentContext(): ProjectContextInfo {
    const basicContext = this.detectBasicProjectInfo();
    const implementationPatterns = this.detectImplementationPatterns();
    
    return {
      ...basicContext,
      implementationPatterns,
      lastUpdated: new Date().toISOString()
    };
  }

  /**
   * Detect basic project information from package.json and file structure
   */
  private detectBasicProjectInfo() {
    const pkg = this.readPackageJson();
    const projectType = this.determineProjectType(pkg);
    const language = this.determineLanguage(pkg);
    const framework = this.determineFramework(pkg);
    const packageManager = this.determinePackageManager();
    
    return {
      projectType,
      language,
      framework,
      packageManager,
      directories: this.mapKeyDirectories(),
      dependencies: Object.keys(pkg.dependencies || {}),
      scripts: pkg.scripts || {}
    };
  }

  /**
   * Core implementation pattern detection
   */
  private detectImplementationPatterns(): ImplementationPatterns {
    return {
      authentication: this.detectAuthImplementation(),
      apiResponses: this.detectAPIResponseImplementation(),
      errorHandling: this.detectErrorHandlingImplementation()
    };
  }

  /**
   * Detect authentication implementation patterns
   */
  private detectAuthImplementation() {
    const authFiles = this.findFiles(['**/auth/**/*.{ts,js}', '**/middleware/auth*.{ts,js}', '**/guards/**/*.{ts,js}']);
    const evidence: string[] = [];
    const signals: PatternDetectionSignal[] = [];
    
    let userProperty = 'Unknown';
    let tokenLocation = 'Unknown';
    let errorFormat = 'Unknown';
    let errorStatusCode = 0;
    let middlewarePattern = 'Unknown';

    if (authFiles.length > 0) {
      for (const file of authFiles.slice(0, 3)) { // Analyze first 3 auth files
        const content = this.readFileContent(file);
        const relativeFile = file.replace(this.projectRoot, '');

        // Detect user property pattern
        if (this.validatePatternInContext(content, 'req.user', ['req', 'user', 'auth'])) {
          userProperty = 'req.user';
          evidence.push(`${relativeFile}: req.user usage`);
          signals.push({ weight: 30, detected: true, evidence: `${relativeFile}: req.user` });
        } else if (this.validatePatternInContext(content, 'req.context.user', ['req', 'context', 'user'])) {
          userProperty = 'req.context.user';
          evidence.push(`${relativeFile}: req.context.user usage`);
          signals.push({ weight: 30, detected: true, evidence: `${relativeFile}: req.context.user` });
        } else if (this.validatePatternInContext(content, 'req.auth', ['req', 'auth'])) {
          userProperty = 'req.auth';
          evidence.push(`${relativeFile}: req.auth usage`);
          signals.push({ weight: 30, detected: true, evidence: `${relativeFile}: req.auth` });
        }

        // Detect token location
        if (content.includes('httpOnly') || content.includes('cookie')) {
          tokenLocation = 'httpOnly cookie';
          evidence.push(`${relativeFile}: cookie-based auth`);
          signals.push({ weight: 25, detected: true });
        } else if (content.includes('authorization') || content.includes('Bearer')) {
          tokenLocation = 'authorization header';
          evidence.push(`${relativeFile}: header-based auth`);
          signals.push({ weight: 25, detected: true });
        }

        // Detect error response format
        if (content.includes('{error:') || content.includes('{ error:')) {
          errorFormat = '{error: string}';
          signals.push({ weight: 20, detected: true });
        } else if (content.includes('{message:') || content.includes('{ message:')) {
          errorFormat = '{message: string}';
          signals.push({ weight: 20, detected: true });
        }

        // Detect status codes
        if (content.includes('401')) {
          errorStatusCode = 401;
          signals.push({ weight: 15, detected: true });
        } else if (content.includes('403')) {
          errorStatusCode = 403;
          signals.push({ weight: 15, detected: true });
        }

        // Detect middleware pattern
        if (content.includes('app.use')) {
          middlewarePattern = 'app.use(auth)';
          signals.push({ weight: 20, detected: true });
        } else if (content.includes('@authenticated') || content.includes('@auth')) {
          middlewarePattern = '@authenticated';
          signals.push({ weight: 20, detected: true });
        }
      }
    } else {
      signals.push({ weight: 30, detected: false }); // No auth files found
    }

    const confidence = this.calculateConfidence(signals);

    return {
      userProperty,
      tokenLocation,
      errorResponse: {
        format: errorFormat,
        statusCode: errorStatusCode
      },
      middlewarePattern,
      confidence: confidence.totalConfidence,
      evidence
    };
  }

  /**
   * Detect API response implementation patterns
   */
  private detectAPIResponseImplementation() {
    const apiFiles = this.findFiles(['**/controllers/**/*.{ts,js}', '**/routes/**/*.{ts,js}', '**/handlers/**/*.{ts,js}']);
    const evidence: string[] = [];
    const signals: PatternDetectionSignal[] = [];
    
    let successFormat = 'Unknown';
    let errorFormat = 'Unknown';
    let statusCodeUsage = 'Unknown';
    let wrapperPattern = 'Unknown';

    if (apiFiles.length > 0) {
      let dataWrapperCount = 0;
      let resultWrapperCount = 0;
      let bareObjectCount = 0;

      // Analyze multiple API files for consistent patterns
      for (const file of apiFiles.slice(0, 3)) {
        const content = this.readFileContent(file);
        const relativeFile = file.replace(this.projectRoot, '');

        // Detect success response format
        if (content.includes('{data:') || content.includes('{ data:')) {
          dataWrapperCount++;
          evidence.push(`${relativeFile}: {data: any} format`);
        } else if (content.includes('{result:') || content.includes('{ result:')) {
          resultWrapperCount++;
          evidence.push(`${relativeFile}: {result: any} format`);
        } else if (content.includes('res.json(') || content.includes('return ')) {
          bareObjectCount++;
          evidence.push(`${relativeFile}: bare object response`);
        }

        // Detect error response format  
        if (content.includes('{error:') || content.includes('{ error:')) {
          errorFormat = '{error: string}';
          evidence.push(`${relativeFile}: {error: string} format`);
          signals.push({ weight: 15, detected: true });
        } else if (content.includes('{message:') || content.includes('{ message:')) {
          errorFormat = '{message: string}';
          evidence.push(`${relativeFile}: {message: string} format`);
          signals.push({ weight: 15, detected: true });
        }

        // Detect status code usage
        if (content.includes('.status(') || content.includes('statusCode')) {
          statusCodeUsage = 'explicit codes';
          signals.push({ weight: 10, detected: true });
        }
      }

      // Determine dominant success format
      if (dataWrapperCount > resultWrapperCount && dataWrapperCount > bareObjectCount) {
        successFormat = '{data: any}';
        wrapperPattern = 'always wrapped';
        signals.push({ weight: 30, detected: true });
      } else if (resultWrapperCount > dataWrapperCount && resultWrapperCount > bareObjectCount) {
        successFormat = '{result: any}';
        wrapperPattern = 'always wrapped';
        signals.push({ weight: 30, detected: true });
      } else if (bareObjectCount > 0) {
        successFormat = 'bare object';
        wrapperPattern = 'conditional';
        signals.push({ weight: 25, detected: true });
      }

      if (statusCodeUsage === 'Unknown') {
        statusCodeUsage = 'default 200/500';
        signals.push({ weight: 5, detected: true });
      }
    } else {
      signals.push({ weight: 30, detected: false }); // No API files found
    }

    const confidence = this.calculateConfidence(signals);

    return {
      successFormat,
      errorFormat,
      statusCodeUsage,
      wrapperPattern,
      confidence: confidence.totalConfidence,
      evidence
    };
  }

  /**
   * Detect error handling implementation patterns
   */
  private detectErrorHandlingImplementation() {
    const errorFiles = this.findFiles(['**/error*.{ts,js}', '**/middleware/error*.{ts,js}', '**/exception*.{ts,js}']);
    const apiFiles = this.findFiles(['**/controllers/**/*.{ts,js}', '**/routes/**/*.{ts,js}']);
    const evidence: string[] = [];
    const signals: PatternDetectionSignal[] = [];
    
    let catchPattern = 'Unknown';
    let errorStructure = 'Unknown';
    let loggingIntegration = 'Unknown';
    let propagationStyle = 'Unknown';

    // Check for global error middleware
    if (errorFiles.length > 0) {
      const errorContent = this.readFileContent(errorFiles[0]);
      const relativeFile = errorFiles[0].replace(this.projectRoot, '');
      
      catchPattern = 'global middleware';
      evidence.push(`${relativeFile}: global error handler`);
      signals.push({ weight: 40, detected: true });

      // Detect error structure from middleware
      if (errorContent.includes('{error:') || errorContent.includes('{ error:')) {
        errorStructure = '{error: string, code?: string}';
        signals.push({ weight: 25, detected: true });
      } else if (errorContent.includes('{message:') || errorContent.includes('{ message:')) {
        errorStructure = '{message: string}';
        signals.push({ weight: 25, detected: true });
      }

      // Check for logging integration
      if (errorContent.includes('console.log') || errorContent.includes('logger') || errorContent.includes('winston')) {
        loggingIntegration = 'integrated';
        signals.push({ weight: 15, detected: true });
      }
    }

    // Check API files for try/catch patterns
    if (apiFiles.length > 0 && catchPattern === 'Unknown') {
      const apiContent = this.readFileContent(apiFiles[0]);
      const relativeFile = apiFiles[0].replace(this.projectRoot, '');
      
      if (apiContent.includes('try {') && apiContent.includes('catch')) {
        catchPattern = 'try/catch blocks';
        evidence.push(`${relativeFile}: try/catch usage`);
        signals.push({ weight: 30, detected: true });
      }

      // Check for Result type patterns
      if (apiContent.includes('Result<') || apiContent.includes('Either<')) {
        catchPattern = 'Result types';
        propagationStyle = 'return errors';
        evidence.push(`${relativeFile}: Result type usage`);
        signals.push({ weight: 35, detected: true });
      } else if (apiContent.includes('throw')) {
        propagationStyle = 'throw exceptions';
        signals.push({ weight: 15, detected: true });
      }
    }

    if (loggingIntegration === 'Unknown') {
      loggingIntegration = 'separate';
      signals.push({ weight: 5, detected: true });
    }

    const confidence = this.calculateConfidence(signals);

    return {
      catchPattern,
      errorStructure,
      loggingIntegration,
      propagationStyle,
      confidence: confidence.totalConfidence,
      evidence
    };
  }

  /**
   * Enhanced pattern matching with context validation
   */
  private validatePatternInContext(content: string, pattern: string, contextKeywords: string[]): boolean {
    const lines = content.split('\n');
    for (let i = 0; i < lines.length; i++) {
      if (lines[i].includes(pattern)) {
        // Check surrounding context for validation
        const contextLines = lines.slice(Math.max(0, i - 2), Math.min(lines.length, i + 3));
        const contextText = contextLines.join(' ');
        return contextKeywords.some(keyword => contextText.includes(keyword));
      }
    }
    return false;
  }

  /**
   * Multi-signal confidence calculation
   */
  private calculateConfidence(signals: PatternDetectionSignal[]): ConfidenceCalculation {
    const totalWeight = signals.reduce((sum, signal) => sum + signal.weight, 0);
    const detectedWeight = signals.reduce((sum, signal) => 
      sum + (signal.detected ? signal.weight : 0), 0);
    
    const totalConfidence = totalWeight > 0 ? Math.round((detectedWeight / totalWeight) * 100) : 0;
    
    let uncertaintyWarning: string | undefined;
    if (totalConfidence < 60) {
      uncertaintyWarning = `⚠️ Pattern unclear (${totalConfidence}% confidence) - Ask before making assumptions`;
    } else if (totalConfidence < 80) {
      uncertaintyWarning = `⚠️ Pattern detected but uncertain (${totalConfidence}% confidence) - Verify before critical decisions`;
    }

    return {
      signals,
      totalConfidence: Math.min(totalConfidence, 100),
      uncertaintyWarning
    };
  }

  /**
   * File system utility methods
   */
  private findFiles(patterns: string[]): string[] {
    const files: string[] = [];
    for (const pattern of patterns) {
      try {
        const matches = glob.sync(pattern, { cwd: this.projectRoot, absolute: true });
        files.push(...matches);
      } catch (error) {
        // Ignore glob errors, continue with other patterns
      }
    }
    return [...new Set(files)]; // Remove duplicates
  }

  private readFileContent(filePath: string): string {
    try {
      return readFileSync(filePath, 'utf8');
    } catch {
      return '';
    }
  }

  private readPackageJson(): any {
    try {
      const pkgPath = join(this.projectRoot, 'package.json');
      return JSON.parse(readFileSync(pkgPath, 'utf8'));
    } catch {
      return {};
    }
  }

  private determineProjectType(pkg: any): string {
    if (pkg.dependencies?.express || pkg.devDependencies?.express) return 'Express API';
    if (pkg.dependencies?.react || pkg.devDependencies?.react) return 'React Application';
    if (pkg.dependencies?.['@nestjs/core']) return 'NestJS API';
    if (pkg.dependencies?.next || pkg.devDependencies?.next) return 'Next.js Application';
    if (pkg.dependencies?.vue || pkg.devDependencies?.vue) return 'Vue Application';
    return 'Node.js Project';
  }

  private determineLanguage(pkg: any): string {
    if (pkg.devDependencies?.typescript || existsSync(join(this.projectRoot, 'tsconfig.json'))) {
      return 'TypeScript';
    }
    return 'JavaScript';
  }

  private determineFramework(pkg: any): string {
    if (pkg.dependencies?.express) return 'Express.js';
    if (pkg.dependencies?.['@nestjs/core']) return 'NestJS';
    if (pkg.dependencies?.react) return 'React';
    if (pkg.dependencies?.next) return 'Next.js';
    if (pkg.dependencies?.vue) return 'Vue.js';
    return 'None detected';
  }

  private determinePackageManager(): string {
    if (existsSync(join(this.projectRoot, 'pnpm-lock.yaml'))) return 'pnpm';
    if (existsSync(join(this.projectRoot, 'yarn.lock'))) return 'yarn';
    if (existsSync(join(this.projectRoot, 'package-lock.json'))) return 'npm';
    return 'npm';
  }

  private mapKeyDirectories(): Array<{path: string, purpose: string}> {
    const dirs = [];
    const checkDirs = [
      { path: 'src', purpose: 'source code' },
      { path: 'src/components', purpose: 'React components' },
      { path: 'src/services', purpose: 'business logic' },
      { path: 'src/middleware', purpose: 'middleware functions' },
      { path: 'src/controllers', purpose: 'API controllers' },
      { path: 'src/routes', purpose: 'API routes' },
      { path: 'src/models', purpose: 'data models' },
      { path: 'src/utils', purpose: 'utility functions' },
      { path: 'src/types', purpose: 'TypeScript types' },
      { path: 'tests', purpose: 'test files' },
      { path: '__tests__', purpose: 'test files' }
    ];

    for (const dir of checkDirs) {
      const fullPath = join(this.projectRoot, dir.path);
      if (existsSync(fullPath) && statSync(fullPath).isDirectory()) {
        dirs.push(dir);
      }
    }

    return dirs;
  }
}