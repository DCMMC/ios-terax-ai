#import <UIKit/UIKit.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>

#include <ctype.h>
#include <atomic>

static std::atomic_bool gTeraxTerminalInputEnabled(false);

static BOOL TeraxTerminalInputEnabled(void) {
    return gTeraxTerminalInputEnabled.load(std::memory_order_acquire);
}

static NSString *TeraxArrow(NSString *key) {
    if ([key isEqualToString:UIKeyInputUpArrow]) return @"\x1b[A";
    if ([key isEqualToString:UIKeyInputDownArrow]) return @"\x1b[B";
    if ([key isEqualToString:UIKeyInputRightArrow]) return @"\x1b[C";
    if ([key isEqualToString:UIKeyInputLeftArrow]) return @"\x1b[D";
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
    NSLog(@"[ios-terminal] native dispatch input length=%lu", (unsigned long)input.length);
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

    for (NSString *special in @[UIKeyInputEscape, UIKeyInputUpArrow, UIKeyInputDownArrow,
                                UIKeyInputLeftArrow, UIKeyInputRightArrow, @"\t"]) {
        [commands addObject:TeraxKeyCommand(special, 0, @selector(terax_terminalKeyCommand:))];
    }

    NSString *controlKeys = @"abcdefghijklmnopqrstuvwxyz@^26-=[]\\ ";
    for (NSUInteger i = 0; i < controlKeys.length; i++) {
        NSString *key = [controlKeys substringWithRange:NSMakeRange(i, 1)];
        [commands addObject:TeraxKeyCommand(key, UIKeyModifierControl, @selector(terax_terminalKeyCommand:))];
    }

    NSString *metaKeys = @"abcdefghijklmnopqrstuvwxyz0123456789-=[]\\;',./";
    for (NSUInteger i = 0; i < metaKeys.length; i++) {
        NSString *key = [metaKeys substringWithRange:NSMakeRange(i, 1)];
        [commands addObject:TeraxKeyCommand(key, UIKeyModifierAlternate, @selector(terax_terminalKeyCommand:))];
    }

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

    NSString *key = command.input ?: @"";
    UIKeyModifierFlags flags = command.modifierFlags;
    NSString *input = nil;

    if (flags == 0) {
        input = TeraxArrow(key);
        if (!input && [key isEqualToString:UIKeyInputEscape]) input = @"\x1b";
        if (!input && [key isEqualToString:@"\t"]) input = @"\t";
    } else if (flags & UIKeyModifierAlternate) {
        input = [@"\x1b" stringByAppendingString:key];
    } else if (flags & UIKeyModifierControl) {
        input = TeraxControlInput(key);
    }

    if (input) {
        TeraxDispatchTerminalInputToWebView(self.webView, input);
    }
}

- (BOOL)canPerformAction:(SEL)action withSender:(id)sender {
    (void)action;
    (void)sender;
    return NO;
}

@end

static TeraxTerminalInputView *gTeraxInputView = nil;

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
}

- (NSArray<UIKeyCommand *> *)terax_keyCommands {
    NSArray<UIKeyCommand *> *commands = [self terax_keyCommands];
    UIKeyCommand *settings = [UIKeyCommand keyCommandWithInput:@","
                                                 modifierFlags:UIKeyModifierCommand
                                                        action:@selector(terax_openSettings:)];
    settings.discoverabilityTitle = @"Open Terax Settings";

    if (commands) {
        return [commands arrayByAddingObject:settings];
    }
    return @[ settings ];
}

- (void)terax_openSettings:(UIKeyCommand *)sender {
    (void)sender;
    [self evaluateJavaScript:@"window.dispatchEvent(new CustomEvent('terax:settings-open',{detail:'general'}));"
           completionHandler:nil];
}

@end
