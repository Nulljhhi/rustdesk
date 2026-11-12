import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import '../../common.dart';
import '../../common/formatter/id_formatter.dart';
import '../../models/server_model.dart';

/// The first command line argument of the process that shows the
/// "connection established" popup on the controlled side.
///
/// The popup runs in its own short-lived process, so it gets its own
/// always-on-top window and does not depend on the connection manager window,
/// which may be hidden (`hide_cm`) or minimized.
const String kConnectionPopupArg = '--connection-popup';

/// How long the popup stays on screen.
const Duration kConnectionPopupDuration = Duration(seconds: 5);

const Size _kConnectionPopupSize = Size(320, 86);

/// Called in the connection manager when a peer has been authorized.
///
/// Everything the popup shows is resolved here (translation, theme), so the
/// popup process does not need to initialize the Rust core.
Future<void> showConnectionPopup(Client client) async {
  if (!isDesktop || desktopType != DesktopType.cm) return;
  try {
    final type = client.isFileTransfer
        ? 'Transfer file'
        : client.isViewCamera
            ? 'View camera'
            : client.isTerminal
                ? 'Terminal'
                : client.portForward.isNotEmpty
                    ? 'Port Forward'
                    : 'Share screen';
    final params = jsonEncode({
      'title': translate('Connection established'),
      'name': client.name,
      'id': formatID(client.peerId),
      'type': translate(type),
      'dark': MyTheme.currentThemeMode() == ThemeMode.dark,
      'compatible_ui': kUseCompatibleUiMode,
    });
    Map<String, String>? environment;
    if (isLinux &&
        (Platform.environment['WAYLAND_DISPLAY'] ?? '').isNotEmpty &&
        (Platform.environment['DISPLAY'] ?? '').isNotEmpty) {
      // Wayland compositors ignore client window positions and "keep above".
      // Use XWayland for this window so it can sit on top in a screen corner.
      environment = {'GDK_BACKEND': 'x11'};
    }
    // Detached: the popup closes itself and must not depend on the CM process.
    await Process.start(
      Platform.resolvedExecutable,
      [kConnectionPopupArg, params],
      environment: environment,
      mode: ProcessStartMode.detached,
    );
  } catch (e) {
    debugPrint('Failed to show connection popup: $e');
  }
}

/// Entry of the popup process, see [kConnectionPopupArg].
Future<void> runConnectionPopup(List<String> args) async {
  // Never linger, even if the window cannot be shown.
  Timer(kConnectionPopupDuration + const Duration(seconds: 10), () => exit(0));
  Map<String, dynamic> params = {};
  try {
    params =
        jsonDecode(args.length > 1 ? args[1] : '{}') as Map<String, dynamic>;
  } catch (e) {
    debugPrint('Failed to parse connection popup params: $e');
  }
  final title = params['title'] as String? ?? 'Connection established';
  await windowManager.ensureInitialized();
  await windowManager.setOpacity(0);
  runApp(_ConnectionPopupApp(title: title, params: params));
  final options = WindowOptions(
    size: _kConnectionPopupSize,
    alwaysOnTop: true,
    skipTaskbar: true,
    title: title,
    // We do not hide titlebar on win7 because of the frame overflow.
    titleBarStyle: params['compatible_ui'] == true
        ? TitleBarStyle.normal
        : TitleBarStyle.hidden,
    windowButtonVisibility: false,
  );
  await windowManager.waitUntilReadyToShow(options, null);
  await windowManager.setResizable(false);
  await windowManager.setAlignment(Alignment.bottomRight);
  if (isMacOS) {
    await windowManager.setVisibleOnAllWorkspaces(true,
        visibleOnFullScreen: true);
  }
  await windowManager.show(inactive: true);
  await windowManager.setOpacity(1);
  Timer(kConnectionPopupDuration, () => exit(0));
}

class _ConnectionPopupApp extends StatelessWidget {
  final String title;
  final Map<String, dynamic> params;

  const _ConnectionPopupApp({required this.title, required this.params});

  @override
  Widget build(BuildContext context) {
    final name = params['name'] as String? ?? '';
    final id = params['id'] as String? ?? '';
    final type = params['type'] as String? ?? '';
    final dark = params['dark'] == true;
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: dark ? MyTheme.darkTheme : MyTheme.lightTheme,
      home: Builder(builder: (context) {
        final theme = Theme.of(context);
        return Scaffold(
          body: Container(
            decoration: BoxDecoration(
              border: Border.all(color: theme.dividerColor),
            ),
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
            child: Row(
              children: [
                const Icon(Icons.check_circle, color: Colors.green, size: 32),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        style: const TextStyle(fontWeight: FontWeight.bold),
                        overflow: TextOverflow.ellipsis,
                      ),
                      if (name.isNotEmpty)
                        Text(name, overflow: TextOverflow.ellipsis),
                      Text(
                        [id, type].where((e) => e.isNotEmpty).join(' · '),
                        style: theme.textTheme.bodySmall,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        );
      }),
    );
  }
}
