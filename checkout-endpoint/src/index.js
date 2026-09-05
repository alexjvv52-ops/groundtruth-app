import { handleCheckout, validateStripeKey } from "./handler.js";

let keyLogged = false;

function logKeyStatus(env) {
  if (keyLogged) return;
  keyLogged = true;
  const check = validateStripeKey(env.STRIPE_RESTRICTED_KEY);
  if (!check.ok) {
    console.log(`Stripe key refused at startup: ${check.reason}`);
  }
}

export default {
  /**
   * @param {Request} request
   * @param {Record<string, string>} env
   * @param {ExecutionContext} _ctx
   */
  async fetch(request, env, _ctx) {
    logKeyStatus(env);
    return handleCheckout(request, env, fetch);
  },
};
