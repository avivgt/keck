// SPDX-License-Identifier: Apache-2.0

import path from "path";
import { ConsoleRemotePlugin } from "@openshift-console/dynamic-plugin-sdk-webpack";

const config = {
  entry: {},
  module: {
    rules: [
      {
        test: /\.tsx?$/,
        use: "ts-loader",
        exclude: /node_modules/,
      },
      {
        test: /\.css$/,
        use: ["style-loader", "css-loader"],
      },
    ],
  },
  resolve: {
    extensions: [".tsx", ".ts", ".js"],
    alias: {
      "@": path.resolve(__dirname, "src"),
    },
  },
  plugins: [new ConsoleRemotePlugin()],
  devServer: {
    port: 9001,
    static: path.join(__dirname, "dist"),
    headers: {
      "Access-Control-Allow-Origin": "*",
    },
  },
};

export default config;
