#import <UIKit/UIKit.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>

#include <ctype.h>
#include <atomic>

static std::atomic_bool gTeraxTerminalInputEnabled(false);
static std::atomic_bool gTeraxApplicationCursor(false);

static BOOL TeraxTerminalInputEnabled(void) {
    return gTeraxTerminalInputEnabled.load(std::memory_order_acquire);
}

static void TeraxDispatchTerminalInputToWebView(WKWebView *webView, NSString *input);

static NSString *TeraxArrow(unichar direction) {
    return [NSString stringWithFormat:@"\x1b%c%C",
                                      gTeraxApplicationCursor.load(std::memory_order_acquire) ? 'O' : '[',
                                      direction];
}

static NSInteger TeraxXtermModifierParam(UIKeyModifierFlags flags) {
    NSInteger param = 1;
    if (flags & UIKeyModifierShift) param += 1;
    if (flags & UIKeyModifierAlternate) param += 2;
    if (flags & UIKeyModifierControl) param += 4;
    return param;
}

static NSString *TeraxCsiFinal(NSString *final, UIKeyModifierFlags flags) {
    NSInteger modifier = TeraxXtermModifierParam(flags);
    if (modifier == 1) return [@"\x1b[" stringByAppendingString:final];
    return [NSString stringWithFormat:@"\x1b[1;%ld%@", (long)modifier, final];
}

static NSString *TeraxTildeKey(NSInteger code, UIKeyModifierFlags flags) {
    NSInteger modifier = TeraxXtermModifierParam(flags);
    if (modifier == 1) return [NSString stringWithFormat:@"\x1b[%ld~", (long)code];
    return [NSString stringWithFormat:@"\x1b[%ld;%ld~", (long)code, (long)modifier];
}

static NSString *TeraxSpecialInput(NSString *key, UIKeyModifierFlags flags) {
    if (flags & UIKeyModifierCommand) return nil;
    if ([key isEqualToString:UIKeyInputEscape]) return @"\x1b";
    if ([key isEqualToString:@"\t"]) {
        return (flags & UIKeyModifierShift) ? @"\x1b[Z" : @"\t";
    }
    if ([key isEqualToString:UIKeyInputUpArrow]) {
        return flags == 0 ? TeraxArrow('A') : TeraxCsiFinal(@"A", flags);
    }
    if ([key isEqualToString:UIKeyInputDownArrow]) {
        return flags == 0 ? TeraxArrow('B') : TeraxCsiFinal(@"B", flags);
    }
    if ([key isEqualToString:UIKeyInputRightArrow]) {
        return flags == 0 ? TeraxArrow('C') : TeraxCsiFinal(@"C", flags);
    }
    if ([key isEqualToString:UIKeyInputLeftArrow]) {
        return flags == 0 ? TeraxArrow('D') : TeraxCsiFinal(@"D", flags);
    }
    if ([key isEqualToString:UIKeyInputHome]) return TeraxCsiFinal(@"H", flags);
    if ([key isEqualToString:UIKeyInputEnd]) return TeraxCsiFinal(@"F", flags);
    if ([key isEqualToString:UIKeyInputPageUp]) return TeraxTildeKey(5, flags);
    if ([key isEqualToString:UIKeyInputPageDown]) return TeraxTildeKey(6, flags);
    if (@available(iOS 15.0, *)) {
        if ([key isEqualToString:UIKeyInputDelete]) return TeraxTildeKey(3, flags);
    }
    if ([key isEqualToString:UIKeyInputF1]) return @"\x1bOP";
    if ([key isEqualToString:UIKeyInputF2]) return @"\x1bOQ";
    if ([key isEqualToString:UIKeyInputF3]) return @"\x1bOR";
    if ([key isEqualToString:UIKeyInputF4]) return @"\x1bOS";
    if ([key isEqualToString:UIKeyInputF5]) return @"\x1b[15~";
    if ([key isEqualToString:UIKeyInputF6]) return @"\x1b[17~";
    if ([key isEqualToString:UIKeyInputF7]) return @"\x1b[18~";
    if ([key isEqualToString:UIKeyInputF8]) return @"\x1b[19~";
    if ([key isEqualToString:UIKeyInputF9]) return @"\x1b[20~";
    if ([key isEqualToString:UIKeyInputF10]) return @"\x1b[21~";
    if ([key isEqualToString:UIKeyInputF11]) return @"\x1b[23~";
    if ([key isEqualToString:UIKeyInputF12]) return @"\x1b[24~";
    return nil;
}

static NSString *TeraxControlInput(NSString *key) {
    if (key.length == 0) return nil;
    unichar ch = [key characterAtIndex:0];
    if (ch == ' ') ch = '\0';
    if (ch == '2') ch = '@';
    if (ch == '6') ch = '^';
    if (ch == '-') ch = '_';

    NSString *allowed = @"abcdefghijklmnopqrstuvwxyz@^26-=[]\\ ";
    if ([allowed rangeOfString:[NSString stringWithCharacters:&ch length:1]].location == NSNotFound) {
        return nil;
    }

    char out = ch == '\0' ? '\0' : (char)(toupper((int)ch) ^ 0x40);
    return [[NSString alloc] initWithBytes:&out length:1 encoding:NSISOLatin1StringEncoding];
}

static UIKeyCommand *TeraxKeyCommand(NSString *input, UIKeyModifierFlags flags, SEL action) {
    UIKeyCommand *command = [UIKeyCommand keyCommandWithInput:input
                                                modifierFlags:flags
                                                       action:action];
    if (@available(iOS 15, *)) {
        command.wantsPriorityOverSystemBehavior = YES;
    }
    return command;
}

static void TeraxAddTerminalKeyCommands(NSMutableArray<UIKeyCommand *> *commands, SEL action) {
    NSMutableArray<NSString *> *specialKeys = [@[
        UIKeyInputEscape, UIKeyInputUpArrow, UIKeyInputDownArrow, UIKeyInputLeftArrow,
        UIKeyInputRightArrow, UIKeyInputPageUp, UIKeyInputPageDown, UIKeyInputHome,
        UIKeyInputEnd, UIKeyInputF1, UIKeyInputF2, UIKeyInputF3, UIKeyInputF4,
        UIKeyInputF5, UIKeyInputF6, UIKeyInputF7, UIKeyInputF8, UIKeyInputF9,
        UIKeyInputF10, UIKeyInputF11, UIKeyInputF12, @"\t"
    ] mutableCopy];
    if (@available(iOS 15.0, *)) {
        [specialKeys addObject:UIKeyInputDelete];
    }

    NSArray<NSNumber *> *specialModifiers = @[
        @0,
        @(UIKeyModifierShift),
        @(UIKeyModifierAlternate),
        @(UIKeyModifierControl),
        @(UIKeyModifierShift | UIKeyModifierAlternate),
        @(UIKeyModifierShift | UIKeyModifierControl),
        @(UIKeyModifierAlternate | UIKeyModifierControl),
        @(UIKeyModifierShift | UIKeyModifierAlternate | UIKeyModifierControl),
    ];
    for (NSString *special in specialKeys) {
        for (NSNumber *modifiers in specialModifiers) {
            if ([special isEqualToString:UIKeyInputEscape] && modifiers.unsignedIntegerValue != 0) continue;
            if ([special isEqualToString:@"\t"] &&
                modifiers.unsignedIntegerValue != 0 &&
                modifiers.unsignedIntegerValue != UIKeyModifierShift) continue;
            [commands addObject:TeraxKeyCommand(special, modifiers.unsignedIntegerValue, action)];
        }
    }

    NSString *controlKeys = @"abcdefghijklmnopqrstuvwxyz@^26-=[]\\ ";
    for (NSUInteger i = 0; i < controlKeys.length; i++) {
        NSString *key = [controlKeys substringWithRange:NSMakeRange(i, 1)];
        [commands addObject:TeraxKeyCommand(key, UIKeyModifierControl, action)];
    }

    NSString *metaKeys = @"abcdefghijklmnopqrstuvwxyz0123456789-=[]\\;',./";
    for (NSUInteger i = 0; i < metaKeys.length; i++) {
        NSString *key = [metaKeys substringWithRange:NSMakeRange(i, 1)];
        [commands addObject:TeraxKeyCommand(key, UIKeyModifierAlternate, action)];
    }
}

static NSString *TeraxInputForKeyCommand(UIKeyCommand *command) {
    NSString *key = command.input ?: @"";
    UIKeyModifierFlags flags = command.modifierFlags;

    NSString *special = TeraxSpecialInput(key, flags);
    if (special) return special;
    if (flags & UIKeyModifierAlternate) {
        return [@"\x1b" stringByAppendingString:key];
    }
    if (flags & UIKeyModifierControl) {
        return TeraxControlInput(key);
    }
    return nil;
}

static NSString *TeraxInputForHardwareKey(NSInteger keyCode, NSString *text, UIKeyModifierFlags flags) {
    if (flags & UIKeyModifierCommand) return nil;

    switch (keyCode) {
        case UIKeyboardHIDUsageKeyboardReturnOrEnter:
        case UIKeyboardHIDUsageKeypadEnter:
            return @"\r";
        case UIKeyboardHIDUsageKeyboardDeleteOrBackspace:
            return @"\x7f";
        case UIKeyboardHIDUsageKeyboardTab:
            return TeraxSpecialInput(@"\t", flags);
        case UIKeyboardHIDUsageKeyboardEscape:
            return TeraxSpecialInput(UIKeyInputEscape, flags);
        case UIKeyboardHIDUsageKeyboardUpArrow:
            return TeraxSpecialInput(UIKeyInputUpArrow, flags);
        case UIKeyboardHIDUsageKeyboardDownArrow:
            return TeraxSpecialInput(UIKeyInputDownArrow, flags);
        case UIKeyboardHIDUsageKeyboardRightArrow:
            return TeraxSpecialInput(UIKeyInputRightArrow, flags);
        case UIKeyboardHIDUsageKeyboardLeftArrow:
            return TeraxSpecialInput(UIKeyInputLeftArrow, flags);
        case UIKeyboardHIDUsageKeyboardHome:
            return TeraxSpecialInput(UIKeyInputHome, flags);
        case UIKeyboardHIDUsageKeyboardEnd:
            return TeraxSpecialInput(UIKeyInputEnd, flags);
        case UIKeyboardHIDUsageKeyboardPageUp:
            return TeraxSpecialInput(UIKeyInputPageUp, flags);
        case UIKeyboardHIDUsageKeyboardPageDown:
            return TeraxSpecialInput(UIKeyInputPageDown, flags);
        case UIKeyboardHIDUsageKeyboardDeleteForward:
            if (@available(iOS 15.0, *)) {
                return TeraxSpecialInput(UIKeyInputDelete, flags);
            }
            return TeraxTildeKey(3, flags);
        case UIKeyboardHIDUsageKeyboardF1:
            return TeraxSpecialInput(UIKeyInputF1, flags);
        case UIKeyboardHIDUsageKeyboardF2:
            return TeraxSpecialInput(UIKeyInputF2, flags);
        case UIKeyboardHIDUsageKeyboardF3:
            return TeraxSpecialInput(UIKeyInputF3, flags);
        case UIKeyboardHIDUsageKeyboardF4:
            return TeraxSpecialInput(UIKeyInputF4, flags);
        case UIKeyboardHIDUsageKeyboardF5:
            return TeraxSpecialInput(UIKeyInputF5, flags);
        case UIKeyboardHIDUsageKeyboardF6:
            return TeraxSpecialInput(UIKeyInputF6, flags);
        case UIKeyboardHIDUsageKeyboardF7:
            return TeraxSpecialInput(UIKeyInputF7, flags);
        case UIKeyboardHIDUsageKeyboardF8:
            return TeraxSpecialInput(UIKeyInputF8, flags);
        case UIKeyboardHIDUsageKeyboardF9:
            return TeraxSpecialInput(UIKeyInputF9, flags);
        case UIKeyboardHIDUsageKeyboardF10:
            return TeraxSpecialInput(UIKeyInputF10, flags);
        case UIKeyboardHIDUsageKeyboardF11:
            return TeraxSpecialInput(UIKeyInputF11, flags);
        case UIKeyboardHIDUsageKeyboardF12:
            return TeraxSpecialInput(UIKeyInputF12, flags);
        default:
            break;
    }

    if (text.length == 0) return nil;
    if (flags & UIKeyModifierAlternate) {
        return [@"\x1b" stringByAppendingString:text];
    }
    if (flags & UIKeyModifierControl) {
        return TeraxControlInput([text lowercaseString]);
    }
    return nil;
}

static NSString *TeraxInputForPress(UIPress *press) {
    if (@available(iOS 13.4, *)) {
        UIKey *key = press.key;
        if (!key) return nil;
        NSString *text = key.charactersIgnoringModifiers ?: key.characters ?: @"";
        return TeraxInputForHardwareKey(key.keyCode, text, key.modifierFlags);
    }
    return nil;
}

static BOOL TeraxHandlePresses(NSSet<UIPress *> *presses, WKWebView *webView) {
    if (!TeraxTerminalInputEnabled()) {
        return NO;
    }
    for (UIPress *press in presses) {
        NSString *input = TeraxInputForPress(press);
        if (input.length > 0) {
            TeraxDispatchTerminalInputToWebView(webView, input);
            return YES;
        }
    }
    return NO;
}

static WKWebView *TeraxFindWebView(UIView *view) {
    if ([view isKindOfClass:WKWebView.class]) {
        return (WKWebView *)view;
    }
    for (UIView *subview in view.subviews) {
        WKWebView *found = TeraxFindWebView(subview);
        if (found) {
            return found;
        }
    }
    return nil;
}

static UIWindow *TeraxKeyWindow(void) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:UIWindowScene.class]) {
            continue;
        }
        UIWindowScene *windowScene = (UIWindowScene *)scene;
        for (UIWindow *window in windowScene.windows) {
            if (window.isKeyWindow) {
                return window;
            }
        }
    }
    return nil;
}

static void TeraxPrepareWebView(WKWebView *webView) {
    if (!webView) {
        return;
    }
    webView.scrollView.scrollEnabled = NO;
    webView.scrollView.delaysContentTouches = NO;
    webView.scrollView.canCancelContentTouches = NO;
    webView.scrollView.panGestureRecognizer.enabled = NO;
}

static void TeraxDispatchTerminalInputToWebView(WKWebView *webView, NSString *input) {
    if (!webView || input.length == 0) {
        return;
    }
    NSDictionary *payload = @{@"input": input};
    NSData *jsonData = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
    NSString *json = [[NSString alloc] initWithData:jsonData encoding:NSUTF8StringEncoding];
    if (json.length == 0) {
        return;
    }
    NSString *script = [NSString stringWithFormat:
        @"window.dispatchEvent(new CustomEvent('terax:native-terminal-input',{detail:(%@).input}));",
        json];
    [webView evaluateJavaScript:script completionHandler:nil];
}

@interface TeraxTerminalInputView : UIView <UIKeyInput>
@property (nonatomic, weak) WKWebView *webView;
@end

@implementation TeraxTerminalInputView

- (instancetype)initWithFrame:(CGRect)frame {
    self = [super initWithFrame:frame];
    if (self) {
        self.backgroundColor = UIColor.clearColor;
        self.alpha = 0.01;
        self.userInteractionEnabled = YES;
        self.inputAssistantItem.leadingBarButtonGroups = @[];
        self.inputAssistantItem.trailingBarButtonGroups = @[];
    }
    return self;
}

- (BOOL)canBecomeFirstResponder {
    return TeraxTerminalInputEnabled();
}

- (BOOL)hasText {
    return YES;
}

- (void)insertText:(NSString *)text {
    if (!TeraxTerminalInputEnabled()) {
        return;
    }
    NSString *input = [text stringByReplacingOccurrencesOfString:@"\n" withString:@"\r"];
    TeraxDispatchTerminalInputToWebView(self.webView, input);
}

- (void)deleteBackward {
    if (!TeraxTerminalInputEnabled()) {
        return;
    }
    TeraxDispatchTerminalInputToWebView(self.webView, @"\x7f");
}

- (UITextSmartDashesType)smartDashesType API_AVAILABLE(ios(11)) {
    return UITextSmartDashesTypeNo;
}

- (UITextSmartQuotesType)smartQuotesType API_AVAILABLE(ios(11)) {
    return UITextSmartQuotesTypeNo;
}

- (UITextSmartInsertDeleteType)smartInsertDeleteType API_AVAILABLE(ios(11)) {
    return UITextSmartInsertDeleteTypeNo;
}

- (UITextAutocapitalizationType)autocapitalizationType {
    return UITextAutocapitalizationTypeNone;
}

- (UITextAutocorrectionType)autocorrectionType {
    return UITextAutocorrectionTypeNo;
}

- (UITextSpellCheckingType)spellCheckingType {
    return UITextSpellCheckingTypeNo;
}

- (NSArray<UIKeyCommand *> *)keyCommands {
    NSMutableArray<UIKeyCommand *> *commands = [NSMutableArray new];
    UIKeyCommand *settings = [UIKeyCommand keyCommandWithInput:@","
                                                 modifierFlags:UIKeyModifierCommand
                                                        action:@selector(terax_openSettings:)];
    settings.discoverabilityTitle = @"Open Terax Settings";
    [commands addObject:settings];

    if (!TeraxTerminalInputEnabled()) {
        return commands;
    }

    TeraxAddTerminalKeyCommands(commands, @selector(terax_terminalKeyCommand:));

    return commands;
}

- (void)terax_openSettings:(UIKeyCommand *)sender {
    (void)sender;
    [self.webView evaluateJavaScript:@"window.dispatchEvent(new CustomEvent('terax:settings-open',{detail:'general'}));"
                    completionHandler:nil];
}

- (void)terax_terminalKeyCommand:(UIKeyCommand *)command {
    if (!TeraxTerminalInputEnabled()) {
        return;
    }

    NSString *input = TeraxInputForKeyCommand(command);

    if (input) {
        TeraxDispatchTerminalInputToWebView(self.webView, input);
    }
}

- (BOOL)canPerformAction:(SEL)action withSender:(id)sender {
    (void)action;
    (void)sender;
    return NO;
}

- (void)pressesBegan:(NSSet<UIPress *> *)presses withEvent:(UIPressesEvent *)event {
    if (TeraxHandlePresses(presses, self.webView)) {
        return;
    }
    [super pressesBegan:presses withEvent:event];
}

@end

static TeraxTerminalInputView *gTeraxInputView = nil;

// Diagnostic: log the exact class+selector of any uncaught ObjC exception
// (e.g. "unrecognized selector sent to instance") to NSLog/syslog before the
// app aborts, so it can be captured via idevicesyslog / crash reports.
static void TeraxUncaughtExceptionHandler(NSException *exception) {
    NSLog(@"[terax-crash] uncaught %@: %@", exception.name, exception.reason);
    for (NSString *frame in exception.callStackSymbols) {
        NSLog(@"[terax-crash]   %@", frame);
    }
}

__attribute__((constructor)) static void TeraxInstallExceptionLogger(void) {
    NSSetUncaughtExceptionHandler(&TeraxUncaughtExceptionHandler);
}

extern "C" void TeraxInstallKeyCommands(void) {}

extern "C" void TeraxSetTerminalInputEnabled(bool enabled) {
    gTeraxTerminalInputEnabled.store(enabled, std::memory_order_release);
    NSLog(@"[ios-terminal] native input enabled=%@", enabled ? @"YES" : @"NO");
    if (!enabled) {
        dispatch_async(dispatch_get_main_queue(), ^{
            [gTeraxInputView resignFirstResponder];
        });
    }
}

extern "C" void TeraxSetTerminalApplicationCursor(bool enabled) {
    gTeraxApplicationCursor.store(enabled, std::memory_order_release);
}

extern "C" void TeraxFocusTerminalInput(void) {
    dispatch_async(dispatch_get_main_queue(), ^{
        UIWindow *keyWindow = TeraxKeyWindow();
        WKWebView *webView = keyWindow ? TeraxFindWebView(keyWindow) : nil;
        TeraxPrepareWebView(webView);
        if (!gTeraxInputView) {
            gTeraxInputView = [[TeraxTerminalInputView alloc] initWithFrame:CGRectMake(-1, -1, 1, 1)];
        }
        gTeraxInputView.webView = webView;
        if (keyWindow && gTeraxInputView.superview != keyWindow) {
            [gTeraxInputView removeFromSuperview];
            [keyWindow addSubview:gTeraxInputView];
        }
        BOOL focused = webView ? [gTeraxInputView becomeFirstResponder] : NO;
        NSLog(@"[ios-terminal] native focus proxy webview=%@ focused=%@",
              webView ? @"YES" : @"NO",
              focused ? @"YES" : @"NO");
    });
}

static void TeraxSwizzleInstanceMethod(Class cls, SEL original, SEL replacement) {
    Method originalMethod = class_getInstanceMethod(cls, original);
    Method replacementMethod = class_getInstanceMethod(cls, replacement);
    if (!replacementMethod) {
        return;
    }

    if (originalMethod &&
        class_addMethod(cls, original, method_getImplementation(replacementMethod),
                        method_getTypeEncoding(replacementMethod))) {
        class_replaceMethod(cls, replacement, method_getImplementation(originalMethod),
                            method_getTypeEncoding(originalMethod));
    } else if (originalMethod) {
        method_exchangeImplementations(originalMethod, replacementMethod);
    }
}

@implementation WKWebView (TeraxKeyCommands)

+ (void)load {
    TeraxSwizzleInstanceMethod(self, @selector(keyCommands), @selector(terax_keyCommands));
    // NOTE: do NOT swizzle pressesBegan:withEvent:. UIResponder's default
    // implementation forwards an unhandled press to the next responder using its
    // own _cmd; invoked through the renamed "terax_pressesBegan:" selector, _cmd
    // becomes terax_pressesBegan:, so UIKit then sends THAT selector to the next
    // responder (a WebKit-internal view) which doesn't implement it -> an
    // "unrecognized selector" SIGABRT on every key event while a terminal tab is
    // open (i.e. typing in the search / new-folder fields crashes). Hardware-key
    // input is covered by the keyCommands swizzle above + the
    // TeraxTerminalInputView focus proxy, which use safe mechanisms.
}

- (NSArray<UIKeyCommand *> *)terax_keyCommands {
    NSArray<UIKeyCommand *> *commands = [self terax_keyCommands];
    NSMutableArray<UIKeyCommand *> *teraxCommands = [NSMutableArray new];
    UIKeyCommand *settings = TeraxKeyCommand(@",", UIKeyModifierCommand, @selector(terax_openSettings:));
    settings.discoverabilityTitle = @"Open Terax Settings";
    [teraxCommands addObject:settings];

    if (TeraxTerminalInputEnabled()) {
        TeraxAddTerminalKeyCommands(teraxCommands, @selector(terax_webTerminalKeyCommand:));
    }
    return commands ? [commands arrayByAddingObjectsFromArray:teraxCommands] : teraxCommands;
}

- (void)terax_openSettings:(UIKeyCommand *)sender {
    (void)sender;
    [self evaluateJavaScript:@"window.dispatchEvent(new CustomEvent('terax:settings-open',{detail:'general'}));"
           completionHandler:nil];
}

- (void)terax_webTerminalKeyCommand:(UIKeyCommand *)command {
    if (!TeraxTerminalInputEnabled()) {
        return;
    }
    NSString *input = TeraxInputForKeyCommand(command);
    if (input) {
        TeraxDispatchTerminalInputToWebView(self, input);
    }
}

@end
