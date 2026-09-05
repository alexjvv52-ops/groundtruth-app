import { handleScan } from "./handler.js";

export default {
  /**
   * @param {Request} request
   * @param {Record<string, any>} env
   * @param {ExecutionContext} _ctx
   */
  async fetch(request, env, _ctx) {
    return handleScan(request, env);
  },
};
