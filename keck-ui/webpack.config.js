// SPDX-License-Identifier: Apache-2.0

const path = require("path");
const { ConsoleRemotePlugin } = require("@openshift-console/dynamic-plugin-sdk-webpack");

module.exports = {
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
