#!/usr/bin/env node
/**
 * Playwright validation: open wallet scorecard and check if "All trades" section exists.
 * Usage: EVALUATOR_AUTH_PASSWORD=<password> node scripts/validate_scorecard_trades.mjs
 *        Or: node scripts/validate_scorecard_trades.mjs  (reads from config via grep)
 */
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { fileURLToPath } from 'url';
import path from 'path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BASE = 'http://localhost:8080';
const WALLET = '0x93b110ff31deb58847e841b3cbc6535b3e7b746e';

function getPassword() {
  if (process.env.EVALUATOR_AUTH_PASSWORD) return process.env.EVALUATOR_AUTH_PASSWORD;
  const configPath = path.join(__dirname, '..', 'config', 'default.toml');
  const content = readFileSync(configPath, 'utf8');
  const match = content.match(/auth_password\s*=\s*"([^"]+)"/);
  if (match && match[1]) return match[1];
  console.error('Set EVALUATOR_AUTH_PASSWORD or ensure config/default.toml has [web] auth_password');
  process.exit(1);
}

async function main() {
  const password = getPassword();
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext();
  const page = await context.newPage();

  try {
    // Login
    await page.goto(`${BASE}/login`, { waitUntil: 'networkidle' });
    const csrf = await page.locator('input[name="csrf_token"]').getAttribute('value');
    if (!csrf) {
      console.log('No login form (auth may be disabled). Going to scorecard.');
    } else {
      await page.fill('input[name="password"]', password);
      await page.click('button[type="submit"]');
      await page.waitForURL(url => url.pathname !== '/login', { timeout: 5000 }).catch(() => {});
    }

    // Wallet scorecard
    await page.goto(`${BASE}/wallet/${WALLET}`, { waitUntil: 'networkidle' });
    const bodyText = await page.locator('body').innerText();
    const hasAllTradesHeading = bodyText.includes('All trades');
    const hasTradesTable = bodyText.includes('Time') && bodyText.includes('Market') && bodyText.includes('Side');
    const hasNoTradesYet = bodyText.includes('No trades for this wallet yet');

    console.log('--- Scorecard page validation ---');
    console.log('URL:', `${BASE}/wallet/${WALLET}`);
    console.log('"All trades" heading present:', hasAllTradesHeading);
    console.log('Trades table headers (Time/Market/Side) present:', hasTradesTable);
    console.log('"No trades for this wallet yet" message:', hasNoTradesYet);
    console.log('Section visible without scroll (in viewport): checking...');

    const allTradesSection = page.locator('h3:has-text("All trades")').first();
    const sectionCount = await allTradesSection.count();
    let inViewport = false;
    if (sectionCount > 0) {
      await allTradesSection.scrollIntoViewIfNeeded();
      inViewport = await allTradesSection.isVisible();
    }
    console.log('"All trades" section in DOM:', sectionCount > 0);
    console.log('"All trades" visible after scroll:', inViewport);

    await page.screenshot({ path: path.join(__dirname, '..', 'scorecard-validation.png'), fullPage: true });
    console.log('Full-page screenshot saved: scorecard-validation.png');

    if (!hasAllTradesHeading) {
      console.log('\nRESULT: "All trades" section NOT FOUND on page. Body snippet:', bodyText.slice(0, 800));
      process.exit(1);
    }
    console.log('\nRESULT: "All trades" section is present on the scorecard page.');
  } finally {
    await browser.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
